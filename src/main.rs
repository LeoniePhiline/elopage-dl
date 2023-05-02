use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use async_recursion::async_recursion;
use clap::Parser;
use color_eyre::{eyre::Context, Result};
use futures::stream::StreamExt;
use reqwest::{
    header::{
        HeaderMap, ACCEPT, ACCEPT_LANGUAGE, AUTHORIZATION, CONTENT_LANGUAGE, DNT, ORIGIN, REFERER,
        USER_AGENT,
    },
    Client,
};
use serde::Deserialize;
use tokio::fs::File;

const USER_AGENT_HEADER: &str =
    "User-Agent: Mozilla/5.0 (X11; Linux x86_64; rv:109.0) Gecko/20100101 Firefox/112.0";
const LANGUAGE_HEADER: &str = "de";

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// The Course ID
    #[arg(short, long)]
    course_id: u32,

    /// The authorization token
    #[arg(short, long)]
    token: String,

    /// Target-dir
    #[arg(short, long)]
    output_dir: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let mut default_headers = HeaderMap::new();

    default_headers.insert(ACCEPT, "application/json".parse()?);
    default_headers.insert(ACCEPT_LANGUAGE, LANGUAGE_HEADER.parse()?);
    default_headers.insert(AUTHORIZATION, args.token.parse()?);
    default_headers.insert(CONTENT_LANGUAGE, LANGUAGE_HEADER.parse()?);
    default_headers.insert(ORIGIN, "https://elopage.com".parse()?);
    default_headers.insert(DNT, "1".parse()?);
    default_headers.insert(REFERER, "https://elopage.com/".parse()?);
    default_headers.insert(USER_AGENT, USER_AGENT_HEADER.parse()?);

    let client = reqwest::ClientBuilder::new()
        .default_headers(default_headers)
        .build()?;

    let course = fetch_course(client.clone(), args.course_id).await?;

    let base_path = PathBuf::from(format!(
        "{}/Elopage/{} ({})/{}",
        args.output_dir,
        safe_path(&course.seller.username),
        safe_path(&course.seller.full_name),
        safe_path(&course.product.name),
    ));

    let lessons_list: Vec<LessonsListItem> = fetch_lessons_list(client.clone(), args.course_id)
        .await?
        .into_iter()
        .filter(|item| item.active)
        .collect();

    let has_categories = lessons_list.iter().any(|item| item.is_category);

    if has_categories {
        // Lessons nested in categories.
        let mut category_paths: BTreeMap<Position, PathBuf> = BTreeMap::new();
        let mut category_positions: BTreeMap<Id, Position> = BTreeMap::new();

        for category in lessons_list.iter().rev().filter(|item| item.is_category) {
            println!("Processing category '{}'", category.id);

            let path = format!("{:0>2} {}", category.position, safe_path(&category.name));
            let path = base_path.join(&path);

            println!("Creating category path '{}'", path.display());
            std::fs::create_dir_all(&path).wrap_err("Failed to create category path")?;

            category_positions.insert(category.id, category.position);
            category_paths.insert(category.position, path);
        }

        for lesson in lessons_list
            .into_iter()
            .rev()
            .filter(|item| !item.is_category)
        {
            println!(
                "Processing lesson '{}' of category '{}'",
                lesson.id,
                lesson.parent_id.unwrap_or_else(|| {
                    println!("No parent ID for {lesson:#?}");
                    panic!();
                })
            );
            let category_path = category_paths
                .get(
                    category_positions
                        .get(
                            &lesson
                                .parent_id
                                .expect("Lesson did not have a parent category ID"),
                        )
                        .expect("Parent category for lesson item not found in module positions"),
                )
                .expect("Parent category for lesson item not found in module tree");

            let path = create_lesson_path(category_path, lesson.position, &lesson.name)?;

            let content_blocks = fetch_lesson_content_blocks(
                client.clone(),
                args.course_id,
                lesson.id,
                lesson.content_page_id.unwrap(),
            )
            .await?;

            download_content_block_assets_recursive(&content_blocks, &path).await?;
        }
    } else {
        // No categories, just plain lessons.
        for lesson in lessons_list.into_iter().rev() {
            println!("Processing lesson '{}'", lesson.id,);

            let path = create_lesson_path(&base_path, lesson.position, &lesson.name)?;

            let content_blocks = fetch_lesson_content_blocks(
                client.clone(),
                args.course_id,
                lesson.id,
                lesson.content_page_id.unwrap(),
            )
            .await?;

            download_content_block_assets_recursive(&content_blocks, &path).await?;
        }
    }

    Ok(())
}

/// Create a path in which the lesson's downloadable assets will be stored.
fn create_lesson_path(base_path: &Path, position: Position, name: &str) -> Result<PathBuf> {
    let path = base_path.join(format!("{:0>2} {}", position, safe_path(name)));

    println!("Creating lesson path '{}'", path.display());
    std::fs::create_dir_all(&path).wrap_err("Failed to create lesson path")?;

    Ok(path)
}

type Id = u32;
type Position = u8;

#[derive(Clone, Debug, Deserialize)]
struct CourseResponse {
    data: Course,
}

#[derive(Clone, Debug, Deserialize)]
struct Course {
    seller: Seller,
    product: Product,
}

#[derive(Clone, Debug, Deserialize)]
struct Seller {
    username: String,
    full_name: String,
}

#[derive(Clone, Debug, Deserialize)]
struct Product {
    name: String,
}

async fn fetch_course(client: Client, course_id: u32) -> Result<Course> {
    let url = format!("https://api.elopage.com/v1/payer/course_sessions/{course_id}");
    let response: CourseResponse = client.get(url).send().await?.json().await?;

    println!("{response:#?}");

    Ok(response.data)
}

#[derive(Clone, Debug, Deserialize)]
struct LessonsListResponse {
    data: LessonsListData,
}

#[derive(Clone, Debug, Deserialize)]
struct LessonsListData {
    list: Vec<LessonsListItem>,
}

#[derive(Clone, Debug, Deserialize)]
struct LessonsListItem {
    id: Id,
    name: String,
    active: bool,
    content_page_id: Option<u32>,
    is_category: bool,
    parent_id: Option<u32>,
    position: Position,
}

/// Fetch the course's lessons list, containing a flat structure of lessons and possibly lesson-parent categories.
async fn fetch_lessons_list(client: Client, course_id: u32) -> Result<Vec<LessonsListItem>> {
    let url = format!("https://api.elopage.com/v1/payer/course_sessions/{course_id}/lessons?page=1&query=&per=10000&sort_key=id&sort_dir=desc&course_session_id={course_id}");
    let response: LessonsListResponse = client.get(url).send().await?.json().await?;

    println!("{response:#?}");

    Ok(response.data.list)
}

#[derive(Clone, Debug, Deserialize)]
struct ContentBlocksResponse {
    data: ContentBlocksData,
}

#[derive(Clone, Debug, Deserialize)]
struct ContentBlocksData {
    content_blocks: Vec<ContentBlock>,
}

#[derive(Clone, Debug, Deserialize)]
struct ContentBlock {
    // id: u32,
    children: Vec<ContentBlock>,
    goods: Option<Vec<Good>>,
}

#[derive(Clone, Debug, Deserialize)]
struct Good {
    digital: DigitalGood,
}

#[derive(Clone, Debug, Deserialize)]
struct DigitalGood {
    // id: u32,
    wistia_data: Option<WistiaData>,
    file: Option<FileAsset>,
}

#[derive(Clone, Debug, Deserialize)]
struct WistiaData {
    // id: u32,
    name: Option<String>,
    r#type: Option<String>,
    assets: Option<Vec<Asset>>,
}

#[derive(Clone, Debug, Deserialize)]
struct Asset {
    url: String,
    #[serde(rename = "fileSize")]
    file_size: usize,
}

#[derive(Clone, Debug, Deserialize)]
struct FileAsset {
    name: Option<String>,
    original: Option<String>,
}

async fn fetch_lesson_content_blocks(
    client: Client,
    course_id: u32,
    lesson_id: u32,
    content_page_id: u32,
) -> Result<Vec<ContentBlock>> {
    let url = format!("https://api.elopage.com/v1/payer/course_sessions/{course_id}/lessons/{lesson_id}/content_pages/{content_page_id}?screen_size=desktop");

    println!("URL: {url}");

    let response: serde_json::Value = client.get(&url).send().await?.json().await?;

    println!("Raw JSON: {response:#?}");

    let response: ContentBlocksResponse = serde_json::from_value(response)?;

    println!("Parsed JSON: {response:#?}");

    Ok(response.data.content_blocks)
}

#[async_recursion(?Send)]
async fn download_content_block_assets_recursive(
    content_blocks: &Vec<ContentBlock>,
    path: &Path,
) -> Result<()> {
    for content_block in content_blocks {
        download_content_block_assets_recursive(&content_block.children, path).await?;

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

async fn download_file(file: &FileAsset, path: &Path) -> Result<()> {
    if let Some(original) = &file.original {
        if original == "https://api.elopage.com/pca/digitals/files/original/missing.png" {
            return Ok(());
        }

        download(original, &file.name, path).await?;
    }

    Ok(())
}

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

async fn download(url: &str, name: &Option<String>, path: &Path) -> Result<()> {
    let parsed_url: reqwest::Url = url.parse()?;
    let name = match name {
        Some(name) => name,
        None => parsed_url
            .path_segments()
            .expect("File URL had no path segments")
            .last()
            .expect("File URL had no last path segment"),
    };
    let path = path.join(safe_path(name));

    println!("Download {} to {}", url, path.display());

    let response = reqwest::get(url).await?;

    let mut file = File::create(path).await?;

    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        tokio::io::copy(&mut chunk?.as_ref(), &mut file).await?;
    }

    Ok(())
}

fn safe_path(s: impl AsRef<str>) -> String {
    s.as_ref()
        .replace('/', "_")
        .replace(':', " - ")
        .replace(['?', '"'], "")
}
