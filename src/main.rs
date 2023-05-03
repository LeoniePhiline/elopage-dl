use std::{
    collections::BTreeMap,
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
use futures::stream::{self, StreamExt, TryStreamExt};
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
    fs::File,
    io::{AsyncBufReadExt, AsyncRead, BufReader},
    process::{Child, Command},
    task::JoinHandle,
};
use tracing::{debug, error, info, instrument, Level};

use crate::args::Args;
use crate::json::*;

mod args;
mod json;
mod trace;

type Id = u32;
type Position = u8;

static REGEX_VIMEO_IFRAME: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"<iframe[^>]* src="(?P<embed_url>https://player\.vimeo\.com/video/[^"]+)""#)
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

    let lessons_list: Vec<LessonsListItem> =
        fetch_lessons_list(authenticated_client.clone(), args.course_id)
            .await?
            .into_iter()
            .filter(|item| item.active)
            .collect();

    let has_categories = lessons_list.iter().any(|item| item.is_category);

    let num_lessons_concurrent = args.parallel.clamp(1, u8::MAX) as usize;

    if has_categories {
        // Lessons nested in categories.
        let mut category_paths: BTreeMap<Position, PathBuf> = BTreeMap::new();
        let mut category_positions: BTreeMap<Id, Position> = BTreeMap::new();

        for category in lessons_list.iter().rev().filter(|item| item.is_category) {
            info!("Processing category ID '{}'...", category.id);

            let path = format!("{:0>2} {}", category.position, safe_path(&category.name));
            let path = base_path.join(&path);

            info!("Creating category path '{}'.", path.display());
            std::fs::create_dir_all(&path).wrap_err("Failed to create category path")?;

            category_positions.insert(category.id, category.position);
            category_paths.insert(category.position, path);

            info!("Finished processing category ID '{}'.", category.id);
        }

        let category_paths = Arc::new(category_paths);
        let category_positions = Arc::new(category_positions);

        stream::iter(
            lessons_list
                .into_iter()
                .rev()
                .filter(|item| !item.is_category),
        )
        .map(|lesson| {
            let category_paths = category_paths.clone();
            let category_positions = category_positions.clone();
            let authenticated_client = authenticated_client.clone();
            let yt_dlp_bin = args.yt_dlp_bin.clone();

            async move {
                info!(
                    "Processing lesson ID '{}' of category ID '{}'...",
                    lesson.id,
                    lesson
                        .parent_id
                        .ok_or_else(|| { eyre!("No parent ID for {lesson:#?}") })?
                );
                let category_path =
                    category_paths
                        .get(
                            category_positions
                                .get(&lesson.parent_id.ok_or_else(|| {
                                    eyre!("Lesson did not have a parent category ID")
                                })?)
                                .ok_or_else(|| {
                                    eyre!(
                                    "Parent category for lesson item not found in module positions"
                                )
                                })?,
                        )
                        .ok_or_else(|| {
                            eyre!("Parent category for lesson item not found in module tree")
                        })?;

                let path = create_lesson_path(category_path, lesson.position, &lesson.name)?;

                let content_blocks = fetch_lesson_content_blocks(
                    authenticated_client.clone(),
                    args.course_id,
                    lesson.id,
                    lesson
                        .content_page_id
                        .ok_or_else(|| eyre!("Lesson had no content page ID"))?,
                )
                .await?;

                download_content_block_assets_recursive(&content_blocks, &path, &yt_dlp_bin)
                    .await?;

                info!(
                    "Finished processing lesson ID '{}' of category ID '{}'.",
                    lesson.id,
                    lesson
                        .parent_id
                        .ok_or_else(|| { eyre!("No parent ID for {lesson:#?}") })?
                );
                Ok::<(), Report>(())
            }
        })
        .buffered(num_lessons_concurrent)
        .try_collect()
        .await?;
    } else {
        let base_path = Arc::new(base_path);

        // No categories, just plain lessons.
        stream::iter(lessons_list.into_iter().rev())
            .map(|lesson| {
                let authenticated_client = authenticated_client.clone();
                let base_path = base_path.clone();
                let yt_dlp_bin = args.yt_dlp_bin.clone();

                async move {
                    info!("Processing lesson ID '{}'...", lesson.id);

                    let path = create_lesson_path(&base_path, lesson.position, &lesson.name)?;

                    let content_blocks = fetch_lesson_content_blocks(
                        authenticated_client.clone(),
                        args.course_id,
                        lesson.id,
                        lesson
                            .content_page_id
                            .ok_or_else(|| eyre!("Lesson had no content page ID"))?,
                    )
                    .await?;

                    download_content_block_assets_recursive(&content_blocks, &path, &yt_dlp_bin)
                        .await?;

                    info!("Finished processing lesson ID '{}'.", lesson.id);

                    Ok::<(), Report>(())
                }
            })
            .buffered(num_lessons_concurrent)
            .try_collect()
            .await?;
    }

    Ok(())
}

/// Create a path in which the lesson's downloadable assets will be stored.
#[instrument(level = Level::DEBUG)]
fn create_lesson_path(base_path: &Path, position: Position, name: &str) -> Result<PathBuf> {
    let path = base_path.join(format!("{:0>2} {}", position, safe_path(name)));

    info!("Creating lesson path '{}'.", path.display());
    std::fs::create_dir_all(&path).wrap_err("Failed to create lesson path")?;

    Ok(path)
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

/// Recurse nested content blocks, discovering and downloading all attached videos and files.
#[async_recursion(?Send)]
#[instrument(level = Level::DEBUG)]
async fn download_content_block_assets_recursive(
    content_blocks: &Vec<ContentBlock>,
    path: &Path,
    yt_dlp_bin: &Path,
) -> Result<()> {
    for content_block in content_blocks {
        download_content_block_assets_recursive(&content_block.children, path, yt_dlp_bin).await?;

        if let Some(content) = &content_block.content.text {
            for captures in REGEX_VIMEO_IFRAME.captures_iter(content) {
                if let Some(embed_url_match) = captures.name("embed_url") {
                    let embed_url =
                        html_escape::decode_html_entities(embed_url_match.as_str()).into_owned();

                    download_embed(embed_url, path, yt_dlp_bin).await?;
                }
            }
        }

        if let Some(goods) = &content_block.goods {
            for good in goods {
                let good = &good.digital;
                if let Some(file) = &good.file {
                    download_file(file, path).await?;
                }

                if let Some(wistia_data) = &good.wistia_data {
                    download_video(wistia_data, path).await?;
                }
            }
        }
    }

    Ok(())
}

/// Download an embedded Vimeo video.
#[instrument(level = Level::DEBUG)]
async fn download_embed(
    embed_url: impl AsRef<OsStr> + Display + Debug,
    path: &Path,
    yt_dlp_bin: &Path,
) -> Result<()> {
    info!("Downloading '{}' to '{}'...", embed_url, path.display());

    child_read_to_end(
        Command::new(yt_dlp_bin)
            .kill_on_drop(true)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .arg("--newline")
            .arg("--no-colors")
            .arg("--add-header")
            .arg("Referer:https://elopage.com/")
            .arg(&embed_url)
            .arg("--paths")
            .arg(path)
            .spawn()
            .wrap_err_with(|| "Command failed to start ({cmd})")?,
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
        .map(|stderr| consume_stream(stderr, |line| error!(line)));

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
    maybe_spawned.map(|join: JoinHandle<Result<()>>| async { join.await? });

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
async fn download_file(file: &FileAsset, path: &Path) -> Result<()> {
    if let Some(original) = &file.original {
        if original == "https://api.elopage.com/pca/digitals/files/original/missing.png" {
            return Ok(());
        }

        download(original, &file.name, path).await?;
    }

    Ok(())
}

/// Stream a video to disk.
#[instrument(level = Level::DEBUG)]
async fn download_video(wistia_data: &WistiaData, path: &Path) -> Result<()> {
    if let Some(assets) = &wistia_data.assets {
        assert!(matches!(wistia_data.r#type.as_deref(), Some("Video")));

        let largest_asset = assets.iter().max_by_key(|asset| asset.file_size);
        // None if assets is empty
        if let Some(asset) = largest_asset {
            download(&asset.url, &wistia_data.name, path).await?;
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

    info!("Downloading '{}' to '{}'", url, path.display());

    let response = reqwest::get(url).await?;

    let mut file = File::create(&path).await?;

    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        tokio::io::copy(&mut chunk?.as_ref(), &mut file).await?;
    }

    info!("Finished downloading '{}' to '{}'", url, path.display());

    Ok(())
}

/// Replace some non path-safe characters for wider file-system compatibility (e.g. with ExFAT).
#[instrument(level = Level::DEBUG)]
fn safe_path(s: impl AsRef<str> + Debug) -> String {
    s.as_ref()
        .replace(": ", " - ")
        .replace('/', "_")
        .replace(['?', '"', ':'], "")
}
