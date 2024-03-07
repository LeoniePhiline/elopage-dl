use std::path::PathBuf;

use clap::Parser;
use reqwest::Url;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
pub(crate) struct Args {
    /// Login email
    #[arg(short, long)]
    pub email: String,

    /// Login password
    #[arg(short, long)]
    pub password: String,

    /// User agent (browser signature)
    #[arg(
        short,
        long,
        env = "USER_AGENT",
        default_value = "Mozilla/5.0 (X11; Linux x86_64; rv:123.0) Gecko/20100101 Firefox/123.0"
    )]
    pub user_agent: String,

    /// Download files of up to N lessons at the same time
    #[arg(short = 'P', long, env = "PARALLEL_DOWNLOADS", default_value_t = 1)]
    pub parallel: usize,

    /// Path to the `yt-dlp` binary - required only if vimeo iframes are used.
    #[arg(short, long, env = "YT_DLP_BIN", default_value = "yt-dlp")]
    pub yt_dlp_bin: PathBuf,

    #[command(flatten)]
    pub verbosity: clap_verbosity_flag::Verbosity,

    /// The URL of the course to be downloaded
    pub course_url: Url,

    /// Target-dir to download into
    pub output_dir: PathBuf,
}
