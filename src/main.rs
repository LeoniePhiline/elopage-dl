use std::{
    ffi::OsStr,
    fmt::{Debug, Display},
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
};

use async_recursion::async_recursion;
use clap::Parser;
use color_eyre::{
    eyre::{eyre, Context},
    Report, Result,
};
use futures::{
    future::BoxFuture,
    stream::{self, BoxStream, StreamExt, TryStreamExt},
    FutureExt,
};
use once_cell::sync::Lazy;
use regex::Regex;
use reqwest::{
    header::{
        HeaderMap, ACCEPT, ACCEPT_LANGUAGE, AUTHORIZATION, CONTENT_LANGUAGE, DNT, ORIGIN, REFERER,
        USER_AGENT,
    },
    Client,
};
use tokio::{
    fs::{create_dir_all, File},
    io::{AsyncBufReadExt, AsyncRead, BufReader},
    process::{Child, Command},
    task::JoinHandle,
};
use tracing::{debug, error, info, instrument, warn, Level};

use crate::args::Args;
use crate::json::*;

mod args;
mod json;
mod trace;

type Id = usize;
type Position = usize;

static REGEX_VIDEO_IFRAME: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"<iframe[^>]* src="(?P<embed_url>https://(?:player\.vimeo\.com/video/|www.youtube.com/embed/)[^"]+)""#)
        .unwrap()
});

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;

    let args = Args::parse();

    trace::init(&args)?;

    let mut default_headers = HeaderMap::new();

    default_headers.insert(ACCEPT, "application/json".parse()?);
    default_headers.insert(ACCEPT_LANGUAGE, args.language.parse()?);
    default_headers.insert(AUTHORIZATION, args.token.parse()?);
    default_headers.insert(CONTENT_LANGUAGE, args.language.parse()?);
    default_headers.insert(ORIGIN, "https://elopage.com".parse()?);
    default_headers.insert(DNT, "1".parse()?);
    default_headers.insert(REFERER, "https://elopage.com/".parse()?);
    default_headers.insert(USER_AGENT, args.user_agent.parse()?);

    let authenticated_client = reqwest::ClientBuilder::new()
        .default_headers(default_headers)
        .build()?;

    let course = fetch_course(authenticated_client.clone(), args.course_id).await?;

    let base_path = PathBuf::from(format!(
        "{}/Elopage/{} ({})/{}",
        args.output_dir,
        safe_path(&course.seller.username),
        safe_path(&course.seller.full_name),
        safe_path(&course.product.name),
    ));

    // Fetch elopage's flat list of lessons and categories.
    let lessons_list: Vec<LessonsListItem> =
        fetch_lessons_list(authenticated_client.clone(), args.course_id)
            .await?
            .into_iter()
            .filter(|item| item.active)
            .collect();

    // Transform the flat list of lessons and categories into a module tree,
    // where both categories and lessons can be either root items, or children of categories.
    let (module_tree, remaining_stack) = resolve_module_tree(None, lessons_list);

    // We expect the entire stack to be part of the tree.
    // `remaining_stack` would be non-empty if an item had a `parent_id` which was either not present in the stack,
    // or it was present, but not a category (but a lesson).
    if !remaining_stack.is_empty() {
        warn!("Remaining stack left over after resolving module tree! Module tree: {module_tree:#?}, remaining stack: {remaining_stack:#?}");
    }

    info!("Resolved module tree.");
    debug!("Module tree: {module_tree:#?}, remaining stack: {remaining_stack:#?}");

    // Some sellers use categories not as containers but merely as separators.
    // We normalize the tree structure by turning "separator categories" into "container categories".
    // To do this, we detect empty root categories and hoist root lessons into preceding empty root categories.
    let module_tree = normalize_module_tree(module_tree);

    // Recurse through the module tree, discovering linked and embedded assets,
    // and create a stream of boxed download futures to process with a user-determined amount of parallelism.
    // TODO: Lesson details are eagerly fetched while processing the tree. (`fetch_lesson_content_blocks`)
    // TODO: It could be nice if they were lazily fetched whenever `StreamExt::buffered` (below) runs empty.
    let downloads_stream = process_tree_recursive(
        module_tree,
        Arc::new(base_path),
        args.course_id,
        authenticated_client,
        Arc::new(args.yt_dlp_bin),
    )
    .await?;

    // Download between 1 and `--parallel` assets in parallel.
    downloads_stream
        .buffered(args.parallel.clamp(1, usize::MAX))
        // Shortcut on download errors.
        .try_collect::<Vec<()>>()
        .await?;

    Ok(())
}

/// Recursively resolve the flat stack of lessons list items into a tree structure by matching the items' `parent_id` propertys.
#[instrument(level = Level::DEBUG)]
fn resolve_module_tree(
    parent_id: Option<Id>,
    stack: Vec<LessonsListItem>,
) -> (Vec<ModuleTreeItem>, Vec<LessonsListItem>) {
    // Extract children of given parent ID (`None` to filter for root items) to push onto the tree,
    // and return the remaining items stack for continued processing.
    let (tree_level_items, mut remaining_stack): (Vec<_>, Vec<_>) = stack
        .into_iter()
        .partition(|item| item.parent_id == parent_id);

    let mut tree_level = Vec::new();

    // Recurse, extracting matching module tree child items from the stack and pushing items onto the tree.
    for item in tree_level_items {
        // Ownership note:
        //
        // `remaining_stack` moves into the recursed function.
        // Remaining items, which were not extracted as child (or grand-child) items,
        // are moved back out of the function, replacing the previous (moved) stack for the next iteration.
        let (children, remaining) = resolve_module_tree(Some(item.id), remaining_stack);
        remaining_stack = remaining;

        // Assuming the observation that items with `is_category == true` do not have other items' `parent_id`s pointing to them.
        // If that turns out to be untrue, then lessons (as opposed to categories) in fact can have children.
        tree_level.push({
            if item.is_category {
                ModuleTreeItem::Category { item, children }
            } else {
                if !children.is_empty() {
                    error!("Collected children for a tree item which is not a category! Children: {children:#?}, Tree item: {item:#?}");
                }
                ModuleTreeItem::Lesson { item }
            }
        })
    }

    // Sort by `position` property.
    tree_level.sort_by(|a, b| {
        match &a {
            ModuleTreeItem::Category { item, .. } => &item.position,
            ModuleTreeItem::Lesson { item } => &item.position,
        }
        .cmp(match &b {
            ModuleTreeItem::Category { item, .. } => &item.position,
            ModuleTreeItem::Lesson { item } => &item.position,
        })
    });

    (tree_level, remaining_stack)
}

/// Normalize the module tree:
/// If an empty root category is directly followed by root lessons, then move these lessons into the empty category.
#[instrument(level = Level::DEBUG)]
fn normalize_module_tree(module_tree: Vec<ModuleTreeItem>) -> Vec<ModuleTreeItem> {
    let mut normalized_tree = Vec::new();
    let mut latest_empty_category = None;
    for tree_item in module_tree.into_iter() {
        let (is_category, is_empty) = match &tree_item {
            ModuleTreeItem::Category { item, children } => {
                if children.is_empty() {
                    warn!("Root category '{}' is empty! Will attempt to collect its supposed children from directly following root-level lessons.", item.name);
                    (true, true)
                } else {
                    (true, false)
                }
            }
            ModuleTreeItem::Lesson { .. } => (false, false),
        };

        // Category - push onto normalized tree and register as latest empty category to attach following root lessons to.
        #[allow(clippy::suspicious_else_formatting)]
        if is_category {
            // All root categories are added to the root of the normalized tree, including empty categories.
            normalized_tree.push(tree_item);

            // Register the index of the latest visited empty category,
            // or reset to `None` if the visited category is not empty.
            if is_empty {
                latest_empty_category = Some(normalized_tree.len() - 1);
            } else {
                latest_empty_category = None;
            }
        } else
        // Lesson - to be pushed into empty category, if present.
        if let Some(empty_category_index) = latest_empty_category {
            // The latest visited category was empty.
            // Take out a mutable reference to it, then push the current lesson into the empty category,
            // instead of adding it to the root of the normalized tree.
            let empty_category = &mut normalized_tree[empty_category_index];
            match empty_category {
                ModuleTreeItem::Category { children, .. } => {
                    children.push(tree_item);
                }
                ModuleTreeItem::Lesson { .. } => {
                    unreachable!("Empty root category can only be a Category enum variant");
                }
            }
        } else {
            // If there was no previously visited category, or the last visited category was not empty,
            // then add the root lesson to the root of the normalized tree.
            normalized_tree.push(tree_item);
        }
    }

    info!("Normalized module tree.");
    debug!("Normalized module tree: {normalized_tree:#?}");

    normalized_tree
}

/// Recursively process the module tree, traversing all categories' children and fetching all lesson assets.
#[async_recursion]
async fn process_tree_recursive(
    module_tree: Vec<ModuleTreeItem>,
    base_path: Arc<PathBuf>,
    course_id: Id,
    authenticated_client: Client,
    yt_dlp_bin: Arc<PathBuf>,
) -> Result<BoxStream<'static, BoxFuture<'static, Result<()>>>> {
    let mut process_tree_stream = stream::iter(module_tree.into_iter().enumerate())
        .then(move |(index, tree_item)| {
            let authenticated_client = authenticated_client.clone();
            let base_path = base_path.clone();
            let yt_dlp_bin = yt_dlp_bin.clone();

            async move {
                match tree_item {
                    ModuleTreeItem::Category {
                        item: category,
                        children,
                    } => {
                        info!("Processing category ID '{}'...", category.id);

                        // Create a category directory, then recurse into children.
                        let path = base_path.join(format!(
                            "{:0>2} {}",
                            index + 1,
                            safe_path(&category.name)
                        ));

                        info!("Creating category path '{}'.", path.display());
                        create_dir_all(&path)
                            .await
                            .wrap_err("Failed to create category path")?;

                        process_tree_recursive(
                            children,
                            Arc::new(path),
                            course_id,
                            authenticated_client,
                            yt_dlp_bin,
                        )
                        .await
                    }
                    ModuleTreeItem::Lesson { item: lesson } => {
                        let log_fmt = format!(
                            "lesson ID '{}'{}...",
                            lesson.id,
                            match lesson.parent_id {
                                Some(parent_id) => format!(" of category ID '{parent_id}'"),
                                None => "".into(),
                            }
                        );
                        info!("Processing {log_fmt}");

                        // Create a path in which the lesson's downloadable assets will be stored, then fetch content blocks and extract assets.
                        let path = base_path.join(format!(
                            "{:0>2} {}",
                            index + 1,
                            safe_path(&lesson.name)
                        ));
                        info!("Creating lesson path '{}'.", path.display());
                        create_dir_all(&path)
                            .await
                            .wrap_err("Failed to create lesson path")?;

                        // Fetch the lesson's nested content blocks structure.
                        let content_blocks = fetch_lesson_content_blocks(
                            authenticated_client,
                            course_id,
                            lesson.id,
                            lesson
                                .content_page_id
                                .ok_or_else(|| eyre!("Lesson had no content page ID"))?,
                        )
                        .await?; // TODO: Can we lazily fetch lessons, driven by downloads stream buffering?

                        // Create a stream of download futures from the lesson's content blocks structure.
                        // Downloadable assets can either be linked to content blocks directly as "goods",
                        // or found as embedded iframes in lesson HTML content.
                        let stream = download_content_block_assets_recursive(
                            content_blocks,
                            Arc::new(path),
                            yt_dlp_bin,
                        );

                        info!("Finished processing {log_fmt}");

                        Ok::<_, Report>(stream)
                    }
                }
            }
        })
        .boxed();

    let mut downloads_stream = stream::iter(Vec::new()).boxed();

    // Handle the result per item (shortcut the discovery stream on error) and then flatten the nested stream.
    // TODO: Here we are driving the tree assets discovery stream, thus downloading lesson details eagerly. Can we do that lazily instead?
    while let Some(next_stream) = process_tree_stream.try_next().await? {
        downloads_stream = downloads_stream.chain(next_stream).boxed();
    }

    Ok(downloads_stream.boxed())
}

/// Fetch a course's metadata.
#[instrument(level = Level::DEBUG)]
async fn fetch_course(authenticated_client: Client, course_id: Id) -> Result<Course> {
    let url = format!("https://api.elopage.com/v1/payer/course_sessions/{course_id}");
    let response: CourseResponse = authenticated_client.get(url).send().await?.json().await?;

    debug!("{response:#?}");

    Ok(response.data)
}

/// Fetch the course's lessons list, containing a flat structure of lessons and possibly lesson-parent categories.
#[instrument(level = Level::DEBUG)]
async fn fetch_lessons_list(
    authenticated_client: Client,
    course_id: Id,
) -> Result<Vec<LessonsListItem>> {
    let url = format!("https://api.elopage.com/v1/payer/course_sessions/{course_id}/lessons?page=1&query=&per=10000&sort_key=id&sort_dir=desc&course_session_id={course_id}");
    let response: LessonsListResponse = authenticated_client.get(url).send().await?.json().await?;

    debug!("{response:#?}");

    Ok(response.data.list)
}

#[instrument(level = Level::DEBUG)]
async fn fetch_lesson_content_blocks(
    authenticated_client: Client,
    course_id: Id,
    lesson_id: Id,
    content_page_id: Id,
) -> Result<Vec<ContentBlock>> {
    let url = format!("https://api.elopage.com/v1/payer/course_sessions/{course_id}/lessons/{lesson_id}/content_pages/{content_page_id}?screen_size=desktop");
    debug!("URL: {url}");

    let response: serde_json::Value = authenticated_client.get(&url).send().await?.json().await?;
    debug!("Raw JSON: {response:#?}");

    let response: ContentBlocksResponse = serde_json::from_value(response)?;
    debug!("Parsed JSON: {response:#?}");

    Ok(response.data.content_blocks)
}

// TODO: Rename functions like these, which no longer actively download, but rather compile a stream of lazy download futures.
/// Recurse nested content blocks, discovering and downloading all attached videos and files.
/// All discovered assets are fed into the same stream, which is returned to the caller.
#[instrument(level = Level::DEBUG)]
fn download_content_block_assets_recursive(
    content_blocks: Vec<ContentBlock>,
    path: Arc<PathBuf>,
    yt_dlp_bin: Arc<PathBuf>,
) -> BoxStream<'static, BoxFuture<'static, Result<()>>> {
    let stream = stream::iter(content_blocks).flat_map(move |content_block| {
        let path = path.clone();
        let yt_dlp_bin = yt_dlp_bin.clone();

        // Recurse into nested content blocks, if any,
        // returning a stream of download futures for all assets discovered in deeper-nested content blocks.
        // If this content block has no children, then an empty stream will be returned, which is immediately ready to yield `None`.
        let mut stream = download_content_block_assets_recursive(
            content_block.children,
            path.clone(),
            yt_dlp_bin.clone(),
        );

        // Chain download futures for assets discovered in the content block's HTML content to the stream returned from recursion.
        if let Some(content) = content_block.content.text {
            // Extract vimeo and youtube embed URLs from this content block's text content.
            let embed_urls = REGEX_VIDEO_IFRAME
                .captures_iter(&content)
                .filter_map(|captures| captures.name("embed_url"))
                .map(|embed_url_match| htmlize::unescape(embed_url_match.as_str()).into_owned())
                .collect::<Vec<_>>();

            // Create a new stream of pinned download futures, and chain it to the stream returned from recursion.
            let path = path.clone();
            stream = stream
                .chain(stream::iter(embed_urls).map(move |embed_url| {
                    let path = path.clone();
                    let yt_dlp_bin = yt_dlp_bin.clone();

                    async move { download_embed(embed_url, path, yt_dlp_bin).await }.boxed()
                }))
                .boxed();
        }

        // Chain download futures for assets directly attached to the content block to the stream returned from recursion.
        if let Some(goods) = content_block.goods {
            stream = stream
                .chain(stream::iter(goods).flat_map(move |good| {
                    let good = good.digital;

                    // None or more downloadable assets ("goods") might be directly attached to the content block.
                    let mut download_futures: Vec<BoxFuture<'static, Result<()>>> = vec![];

                    // Files can be streamed to disk by URL.
                    if let Some(file) = good.file {
                        let path = path.clone();
                        download_futures
                            .push(async move { download_file(file, path).await }.boxed());
                    }

                    // Wistia videos can be streamed to disk after discovering the URL to the largest version of the video.
                    if let Some(wistia_data) = good.wistia_data {
                        let path = path.clone();
                        download_futures
                            .push(async move { download_video(wistia_data, path).await }.boxed());
                    }

                    stream::iter(download_futures).boxed()
                }))
                .boxed();
        }

        stream
    });

    Box::pin(stream)
}

/// Download an embedded Vimeo video.
#[instrument(level = Level::DEBUG)]
async fn download_embed(
    embed_url: impl AsRef<OsStr> + Display + Debug,
    path: Arc<PathBuf>,
    yt_dlp_bin: Arc<PathBuf>,
) -> Result<()> {
    info!("Downloading '{}' to '{}'...", embed_url, path.display());

    // Spawn a task handling the child process,
    // and read piped IO streams into trace logs.
    child_read_to_end(
        Command::new(&*yt_dlp_bin)
            .kill_on_drop(true)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .arg("--newline")
            .arg("--no-colors")
            .arg("--legacy-server-connect")
            .arg("--add-header")
            .arg("Referer:https://elopage.com/")
            .arg(&embed_url)
            .arg("--paths")
            .arg(&*path)
            .spawn()
            .wrap_err("yt-dlp command failed to start")?,
    )
    .await?;

    info!(
        "Finished downloading '{}' to '{}'.",
        embed_url,
        path.display()
    );

    Ok(())
}

/// Spawn a child process and read its stdout and stderr streams to their end.
#[instrument(level = Level::DEBUG)]
async fn child_read_to_end(mut child: Child) -> Result<()> {
    let consume_stdout = child
        .stdout
        .take()
        .map(|stdout| consume_stream(stdout, |line| debug!(line)));

    let consume_stderr = child
        .stderr
        .take()
        .map(|stderr| consume_stream(stderr, |line| warn!(line)));

    let await_exit = async {
        tokio::spawn(async move {
            child
                .wait()
                .await
                .wrap_err("yt-dlp command failed to run")?;

            Ok::<(), Report>(())
        })
        .await??;

        Ok(())
    };

    tokio::try_join!(
        maybe_join(consume_stdout),
        maybe_join(consume_stderr),
        await_exit,
    )
    .wrap_err("Could not join child consumers for stdout, stderr and awaiting child exit.")?;

    Ok(())
}

// Await the `JoinHandle` if the given `Option` is `Some(_)`
#[inline]
async fn maybe_join(maybe_spawned: Option<JoinHandle<Result<()>>>) -> Result<()> {
    if let Some(spawned) = maybe_spawned {
        return spawned.await?;
    }

    Ok(())
}

/// Consume a child process stream, invoking a callback on each line.
#[instrument(level = Level::DEBUG)]
fn consume_stream<A: AsyncRead + Unpin + Send + 'static + Debug>(
    reader: A,
    callback: fn(String),
) -> JoinHandle<Result<()>> {
    let mut lines = BufReader::new(reader).lines();

    tokio::spawn(async move {
        while let Some(line) = lines.next_line().await? {
            callback(line);
        }

        Ok::<(), Report>(())
    })
}

/// Stream a file asset to disk.
#[instrument(level = Level::DEBUG)]
async fn download_file(file: FileAsset, path: Arc<PathBuf>) -> Result<()> {
    if let Some(original) = &file.original {
        if original == "https://api.elopage.com/pca/digitals/files/original/missing.png" {
            return Ok(());
        }

        download(original, &file.name, &path).await?;
    }

    Ok(())
}

/// Stream a video to disk.
#[instrument(level = Level::DEBUG)]
async fn download_video(wistia_data: WistiaData, path: Arc<PathBuf>) -> Result<()> {
    if let Some(assets) = &wistia_data.assets {
        assert!(matches!(wistia_data.r#type.as_deref(), Some("Video")));

        let largest_asset = assets.iter().max_by_key(|asset| asset.file_size);
        // None if assets is empty
        if let Some(asset) = largest_asset {
            download(&asset.url, &wistia_data.name, &path).await?;
        }
    }

    Ok(())
}

/// Stream a video or file to disk.
#[instrument(level = Level::DEBUG)]
async fn download(url: &str, name: &Option<String>, path: &Path) -> Result<()> {
    let parsed_url: reqwest::Url = url.parse()?;
    let name = match name {
        Some(name) => name,
        None => parsed_url
            .path_segments()
            .ok_or_else(|| eyre!("File URL had no path segments"))?
            .last()
            .ok_or_else(|| eyre!("File URL had no last path segment"))?,
    };
    let path = path.join(safe_path(name));

    info!("Downloading '{}' to '{}'...", url, path.display());

    let response = reqwest::get(url).await?;

    let mut file = File::create(&path).await?;

    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        tokio::io::copy(&mut chunk?.as_ref(), &mut file).await?;
    }

    info!("Finished downloading '{}' to '{}'.", url, path.display());

    Ok(())
}

/// Replace some non path-safe characters for wider file-system compatibility (e.g. with ExFAT).
#[instrument(level = Level::DEBUG)]
fn safe_path(s: impl AsRef<str> + Debug) -> String {
    htmlize::unescape(s.as_ref())
        .replace(": ", " - ")
        .replace(" / ", " - ")
        .replace('/', " - ")
        .replace('*', "-")
        .replace(['?', '"', ':'], "")
        .trim()
        .to_owned()
}
