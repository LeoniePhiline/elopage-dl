use std::{
    borrow::Cow,
    collections::HashMap,
    ffi::OsStr,
    fmt::{Debug, Display},
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
};

use clap::Parser;
use color_eyre::{
    eyre::{Context, OptionExt},
    Report, Result,
};
use futures::{
    future::BoxFuture,
    stream::{self, BoxStream, StreamExt, TryStreamExt},
    FutureExt,
};
use once_cell::sync::Lazy;
use regex::{Captures, Regex};
use reqwest::{
    header::{HeaderMap, ACCEPT, USER_AGENT},
    Client, Url,
};
use tokio::{
    fs::File,
    io::{AsyncBufReadExt, AsyncRead, BufReader},
    process::{Child, Command},
    task::JoinHandle,
};
use tracing::{debug, info, instrument, warn, Level};

use crate::args::Args;

mod args;
mod trace;

static REGEX_COURSE_TITLE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"<title>(?P<title>.+?)</title>").unwrap());

static REGEX_EMBED_VIDEO: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"<iframe[^>]* (?:data-)?src="(?P<embed_url>https://(?:player\.vimeo\.com/video/|www\.youtube(?:-nocookie)?\.com/embed/).+?)""#)
        .unwrap()
});

static REGEX_IFRAME_METADATA: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"data-iframe-url="(?P<iframe_url>.+?)""#).unwrap());

static REGEX_IFRAME_HLS: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"hlsUrl: "(?P<hls_url>.+?)","#).unwrap());

static REGEX_LECTURE_GROUPS: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"<a href="(?P<url>.+?)" class="main-lecture.+?">\s+<div class="title">(?P<title>.+?)</div>"#).unwrap()
});

static REGEX_LECTURES: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"<a href="(?P<url>.+?)" class="sub-lecture.+?">\s+<div class="title">(?P<title>.+?)</div>"#).unwrap()
});

static REGEX_LECTURE_DOWNLOADS: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"<a href="(?P<download_url>.+?)"(?: \w+=".*?")*? class="download" download="(?P<file_name>.+?)">"#).unwrap()
});

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;

    let args = Args::parse();

    trace::init(&args)?;

    // TODO: Can Clap use Arcs?
    let authenticated_client = authenticate(
        &args.course_url,
        &args.email,
        &args.password,
        &args.user_agent,
    )
    .await?;

    let course_url = Arc::new(args.course_url);
    let yt_dlp_args = Arc::new(args.yt_dlp_bin);

    let downloads_stream = discover_downloads(
        authenticated_client,
        course_url,
        &args.output_dir,
        args.parallel,
        yt_dlp_args,
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

async fn authenticate(
    course_url: &Url,
    email: &str,
    password: &str,
    user_agent: &str,
) -> Result<Client> {
    let client = reqwest::Client::builder().cookie_store(true).build()?;

    let request = client
        .post({
            let mut login_url = course_url.clone();
            login_url.set_path("action/authenticate");
            debug!(login_url = login_url.as_str());
            login_url
        })
        .headers({
            let mut headers = HeaderMap::new();
            headers.insert(ACCEPT, "*/*".parse()?);
            headers.insert(USER_AGENT, user_agent.parse()?);
            debug!(?headers);
            headers
        })
        .form(&{
            let mut form_data = HashMap::new();
            form_data.insert("email", email);
            form_data.insert("password", password);
            debug!(?form_data);
            form_data
        })
        .build()?;

    info!("Authenticating...");

    debug!(?request);
    let _response = client.execute(request).await?;

    Ok(client)
}

async fn discover_downloads(
    authenticated_client: Client,
    course_url: Arc<Url>,
    output_dir: &Path,
    parallel: usize,
    yt_dlp_args: Arc<PathBuf>,
) -> Result<BoxStream<'static, BoxFuture<'static, Result<()>>>> {
    // Enqueue course root page downloads,
    // and extract lecture URLs for further downloads discovery.
    let (lectures_mapped, downloads_stream) = process_course_root_page(
        authenticated_client.clone(),
        (*course_url).clone(),
        output_dir,
        Arc::clone(&yt_dlp_args),
    )
    .await?;

    // Traverse lecture URLs and enqueue downloads for each lecture.
    let downloads_stream =
        stream::iter(
            lectures_mapped
                .into_iter()
                .map(|(lecture_output_path, lecture_url)| {
                    let authenticated_client = authenticated_client.clone();
                    let yt_dlp_args = Arc::clone(&yt_dlp_args);

                    process_lecture_page(
                        authenticated_client,
                        lecture_url,
                        lecture_output_path,
                        yt_dlp_args,
                    )
                }),
        )
        // Process between 1 and `--parallel` lecture pages in parallel.
        .buffered(parallel.clamp(1, usize::MAX))
        .try_fold(
            downloads_stream,
            |downloads_stream, lecture_downloads_stream| {
                async move {
                    Result::<
                            BoxStream<'static, BoxFuture<'static, Result<()>>>,
                            color_eyre::Report,
                        >::Ok(
                            // Extend the downloads queue with downloads
                            // which are newly discovered at the provided lecture URL.
                            downloads_stream.chain(lecture_downloads_stream).boxed(),
                        )
                }
            },
        )
        .await?;

    Ok(downloads_stream)
}

async fn process_course_root_page(
    authenticated_client: Client,
    course_url: Url,
    output_dir: &Path,
    yt_dlp_args: Arc<PathBuf>,
) -> Result<(
    Vec<(PathBuf, Url)>,
    BoxStream<'static, BoxFuture<'static, Result<()>>>,
)> {
    info!(%course_url, ?output_dir, "Processing course page");

    // Fetch course HTML
    let content = authenticated_client
        .get(course_url.clone())
        .send()
        .await?
        .text()
        .await?;

    // Determine course title
    let course_title = REGEX_COURSE_TITLE
        .captures(&content)
        .and_then(|captures| captures.name("title"))
        .map(|title_match| htmlize::unescape_attribute(title_match.as_str()))
        .unwrap_or_else(|| Cow::Borrowed(course_url.path().trim_start_matches('/')));

    let output_path = Arc::new(output_dir.join(safe_path(course_title)));
    let referer_url = Arc::new(course_url.as_str().to_owned());

    // Create output dir
    tokio::fs::create_dir_all(output_path.as_ref()).await?;

    // Initialize the global downloads queue by enqueueing
    // any downloadable embeds found directly on the course root page.
    let downloads_stream = enqueue_embeds(
        &content,
        Arc::clone(&referer_url),
        Arc::clone(&output_path),
        Arc::clone(&yt_dlp_args),
    )
    .await;

    // If there are downloadable iframe videos, enqueue them for download.
    let downloads_stream = downloads_stream
        .chain(
            enqueue_iframes(
                &content,
                authenticated_client.clone(),
                Arc::clone(&referer_url),
                Arc::clone(&output_path),
                yt_dlp_args,
            )
            .await?,
        )
        .boxed();

    // Are there any downloadable files on the course root page? If so, then extract these as well.
    let downloads_stream = downloads_stream
        .chain(
            enqueue_downloads(
                &content,
                authenticated_client.clone(),
                referer_url,
                Arc::clone(&output_path),
            )
            .await?,
        )
        .boxed();

    fn filter_map(captures: Captures<'_>) -> Option<Result<(Url, Cow<'_, str>)>> {
        captures.name("url").and_then(|group_url| {
            captures.name("title").map(|group_title| {
                let group_url = htmlize::unescape_attribute(group_url.as_str().trim());
                Ok((
                    Url::parse(&group_url)
                        .wrap_err_with(|| format!("failed to parse group URL '{group_url}'"))?,
                    htmlize::unescape_attribute(group_title.as_str().trim()),
                ))
            })
        })
    }

    // Extract lecture groups
    let lecture_groups = REGEX_LECTURE_GROUPS
        .captures_iter(&content)
        .filter_map(filter_map)
        .collect::<Result<Vec<_>>>()?;

    debug!(?lecture_groups);

    // Extract lecture URLs
    let lectures = REGEX_LECTURES
        .captures_iter(&content)
        .filter_map(filter_map)
        .collect::<Result<Vec<_>>>()?;

    debug!(?lectures);

    // Map group and lecture titles and their directory indices to paths
    let lectures_mapped = lecture_groups
        .into_iter()
        .enumerate()
        .flat_map(|(group_index, (group_url, group_title))| {
            let group_path =
                output_path.join(safe_path(format!("{:0>2} {group_title}", group_index + 1)));

            // Group lectures
            lectures
                .iter()
                .filter(move |(lecture_url, _)| {
                    lecture_url.as_str().starts_with(group_url.as_str())
                })
                .enumerate()
                .map(move |(lecture_index, (lecture_url, lecture_title))| {
                    (
                        // Nested output path
                        group_path.join(safe_path(format!(
                            "{:0>2} {lecture_title}",
                            lecture_index + 1
                        ))),
                        // Lecture URL
                        lecture_url.clone(),
                    )
                })
        })
        .collect::<Vec<_>>();

    debug!(?lectures_mapped);

    Ok((lectures_mapped, downloads_stream))
}

async fn process_lecture_page(
    authenticated_client: Client,
    lecture_url: Url,
    output_path: PathBuf,
    yt_dlp_args: Arc<PathBuf>,
) -> Result<BoxStream<'static, BoxFuture<'static, Result<()>>>> {
    info!(%lecture_url, ?output_path, "Processing lecture page");

    let referer_url = Arc::new(lecture_url.as_str().to_owned());
    let output_path = Arc::new(output_path);

    // Fetch lecture HTML
    let content = authenticated_client
        .get(lecture_url)
        .send()
        .await?
        .text()
        .await?;

    // Create output dir
    tokio::fs::create_dir_all(output_path.as_ref()).await?;
    let downloads_stream = enqueue_embeds(
        &content,
        Arc::clone(&referer_url),
        Arc::clone(&output_path),
        Arc::clone(&yt_dlp_args),
    )
    .await;

    let downloads_stream = downloads_stream
        .chain(
            enqueue_iframes(
                &content,
                authenticated_client.clone(),
                Arc::clone(&referer_url),
                Arc::clone(&output_path),
                yt_dlp_args,
            )
            .await?,
        )
        .boxed();

    let downloads_stream = downloads_stream
        .chain(
            enqueue_downloads(
                &content,
                authenticated_client.clone(),
                referer_url,
                output_path,
            )
            .await?,
        )
        .boxed();

    Ok(downloads_stream)
}

async fn enqueue_embeds(
    content: &str,
    referer_url: Arc<String>,
    output_path: Arc<PathBuf>,
    yt_dlp_bin: Arc<PathBuf>,
) -> BoxStream<'static, BoxFuture<'static, Result<()>>> {
    // Extract vimeo and youtube embed URLs.
    let embed_urls = REGEX_EMBED_VIDEO
        .captures_iter(content)
        .filter_map(|captures| captures.name("embed_url"))
        .map(|embed_url_match| htmlize::unescape_attribute(embed_url_match.as_str()).into_owned())
        .collect::<Vec<_>>();

    // Create a new stream of pinned download futures, and chain it to the stream returned from recursion.
    stream::iter(embed_urls)
        .map(move |embed_url| {
            info!(
                %embed_url,
                "Enqueueing video embed"
            );
            let output_path = Arc::clone(&output_path);
            let yt_dlp_bin = Arc::clone(&yt_dlp_bin);
            let referer_url = Arc::clone(&referer_url);
            async move { download_ytdlp(embed_url, &referer_url, &output_path, &yt_dlp_bin).await }
                .boxed()
        })
        .boxed()
}

async fn enqueue_iframes(
    content: &str,
    authenticated_client: Client,
    referer_url: Arc<String>,
    output_path: Arc<PathBuf>,
    yt_dlp_bin: Arc<PathBuf>,
) -> Result<BoxStream<'static, BoxFuture<'static, Result<()>>>> {
    // Extract api.vhs.live.ds25.io iframe video player URLs.
    let iframe_metadata_urls = REGEX_IFRAME_METADATA
        .captures_iter(content)
        .filter_map(|captures| captures.name("iframe_url"))
        .map(|iframe_url_match| htmlize::unescape_attribute(iframe_url_match.as_str()).into_owned())
        .collect::<Vec<_>>();

    debug!(?iframe_metadata_urls);

    let hls_urls = stream::iter(iframe_metadata_urls.into_iter().map(Result::Ok))
        .and_then(|iframe_metadata_url| async {
            // Fetch iframe metadata.
            let metadata = authenticated_client
                .get(iframe_metadata_url)
                .send()
                .await?
                .text()
                .await?;

            // Extract URL from captures.
            // There was no need for match str unescaping, last I checked.
            let hls_url = REGEX_IFRAME_HLS
                .captures(&metadata)
                .ok_or_eyre("failed to find HLS URL in iframe metadata")?
                .name("hls_url")
                .ok_or_eyre("failed to extract HLS URL from iframe metadata")?
                .as_str();

            Result::<_, Report>::Ok(hls_url.to_owned())
        })
        .try_collect::<Vec<_>>()
        .await?;

    debug!(?hls_urls);

    // Create a new stream of pinned download futures, and chain it to the stream returned from recursion.
    Ok(stream::iter(hls_urls)
        .map(move |hls_url| {
            info!(
                %hls_url,
                "Enqueueing video iframe"
            );
            let output_path = Arc::clone(&output_path);
            let yt_dlp_bin = Arc::clone(&yt_dlp_bin);
            let referer_url = Arc::clone(&referer_url);
            async move { download_ytdlp(hls_url, &referer_url, &output_path, &yt_dlp_bin).await }
                .boxed()
        })
        .boxed())
}

async fn enqueue_downloads(
    content: &str,
    authenticated_client: Client,
    referer_url: Arc<String>,
    output_path: Arc<PathBuf>,
) -> Result<BoxStream<'static, BoxFuture<'static, Result<()>>>> {
    let downloads = REGEX_LECTURE_DOWNLOADS
        .captures_iter(content)
        .enumerate()
        .filter_map(|(download_index, captures)| {
            captures.name("download_url").and_then(|download_url| {
                captures.name("file_name").map(|file_name| {
                    let download_url = htmlize::unescape_attribute(download_url.as_str().trim());
                    Ok((
                        Url::parse(&download_url).wrap_err_with(|| {
                            format!("failed to parse download URL '{download_url}'")
                        })?,
                        format!(
                            "{:0>2} {}",
                            download_index + 1,
                            htmlize::unescape_attribute(file_name.as_str().trim())
                        ),
                    ))
                })
            })
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(stream::iter(downloads)
        .map(move |(download_url, file_name)| {
            info!(%download_url, %file_name, "Enqueueing file download");
            let output_path = Arc::clone(&output_path);
            let authenticated_client = authenticated_client.clone();
            let referer_url = Arc::clone(&referer_url);
            async move {
                download_file(
                    download_url,
                    &file_name,
                    authenticated_client,
                    &referer_url,
                    &output_path,
                )
                .await
            }
            .boxed()
        })
        .boxed())
}

/// Download an embedded Vimeo video.
#[instrument(level = Level::DEBUG)]
async fn download_ytdlp(
    embed_url: impl AsRef<OsStr> + Display + Debug,
    referer_url: &str,
    output_path: &Path,
    yt_dlp_bin: &Path,
) -> Result<()> {
    info!(
        "Downloading video '{}' to '{}'...",
        embed_url,
        output_path.display()
    );

    // Spawn a task handling the child process,
    // and read piped IO streams into trace logs.
    child_read_to_end(
        Command::new(yt_dlp_bin)
            .kill_on_drop(true)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .arg("--newline")
            .arg("--no-colors")
            .arg("--legacy-server-connect")
            .arg("--add-header")
            .arg(format!("Referer:{}", referer_url))
            .arg(&embed_url)
            .arg("--paths")
            .arg(output_path)
            .spawn()
            .wrap_err("yt-dlp command failed to start")?,
    )
    .await?;

    info!(
        "Finished downloading '{}' to '{}'.",
        embed_url,
        output_path.display()
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

/// Stream a video or file to disk.
#[instrument(level = Level::DEBUG)]
async fn download_file(
    download_url: Url,
    file_name: &str,
    authenticated_client: Client,
    referer_url: &str,
    output_path: &Path,
) -> Result<()> {
    let file_path = output_path.join(safe_path(file_name));

    info!(
        "Downloading file '{}' to '{}'...",
        download_url,
        file_path.display()
    );

    let response = authenticated_client
        .get(download_url.clone())
        .send()
        .await?;

    let mut file = File::create(&file_path).await?;

    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        tokio::io::copy(&mut chunk?.as_ref(), &mut file).await?;
    }

    info!(
        "Finished downloading '{}' to '{}'.",
        download_url,
        file_path.display()
    );

    Ok(())
}

/// Replace some non path-safe characters for wider file-system compatibility (e.g. with ExFAT).
/// Truncate the path segment if necessary.
#[instrument(level = Level::DEBUG)]
fn safe_path(s: impl AsRef<str> + Debug) -> String {
    let path = htmlize::unescape(s.as_ref())
        .replace(": ", " - ")
        .replace(" / ", " - ")
        .replace('/', " - ")
        .trim()
        .replace('*', "-")
        .replace(['?', '"', ':'], "");

    // Ellipsis if more than 255 chars.
    // This probably has some edge cases with ExFat encoding, UTF-16LE, chars vs bytes vs graphemes.
    match path.char_indices().nth(197) {
        None => path,
        Some((idx, _)) if path.chars().count() > 200 => format!("{}...", &path[..idx]),
        Some(_) => path,
    }
}
