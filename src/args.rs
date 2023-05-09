use std::path::PathBuf;

use clap::Parser;

use crate::Id;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
pub(crate) struct Args {
    /// The Course ID
    #[arg(short, long, env = "COURSE_ID")]
    pub course_id: Id,

    /// The authorization token
    #[arg(short, long, env = "AUTH_TOKEN")]
    pub token: String,

    /// Target-dir
    #[arg(short, long, env = "ELOPAGE_DIR")]
    pub output_dir: String,

    /// User agent (browser signature)
    #[arg(
        short,
        long,
        env = "USER_AGENT",
        default_value = "User-Agent: Mozilla/5.0 (X11; Linux x86_64; rv:109.0) Gecko/20100101 Firefox/112.0"
    )]
    pub user_agent: String,

    /// Content language tag, such as "fr", "de-CH" or "en-CA"
    #[arg(short, long, env = "CONTENT_LANGUAGE", default_value = "en")]
    pub language: String,

    /// Download files of up to N lessons at the same time
    #[arg(short, long, env = "PARALLEL_DOWNLOADS", default_value_t = 1)]
    pub parallel: usize,

    /// Path to the `yt-dlp` binary - required only if vimeo iframes are used.
    #[arg(short, long, env = "YT_DLP_BIN", default_value = "yt-dlp")]
    pub yt_dlp_bin: PathBuf,

    #[command(flatten)]
    pub verbosity: clap_verbosity_flag::Verbosity,
}
