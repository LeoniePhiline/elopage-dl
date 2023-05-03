use serde::Deserialize;

use crate::{Id, Position};

// == Fetch course ==

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct CourseResponse {
    pub data: Course,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Course {
    pub seller: Seller,
    pub product: Product,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Seller {
    pub username: String,
    pub full_name: String,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Product {
    pub name: String,
}

// == Fetch lessons list ==

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct LessonsListResponse {
    pub data: LessonsListData,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct LessonsListData {
    pub list: Vec<LessonsListItem>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct LessonsListItem {
    pub id: Id,
    pub name: String,
    pub active: bool,
    pub content_page_id: Option<Id>,
    pub is_category: bool,
    pub parent_id: Option<Id>,
    pub position: Position,
}

#[derive(Clone, Debug)]
pub(crate) enum ModuleTreeItem {
    Category {
        item: LessonsListItem,
        children: Vec<ModuleTreeItem>,
    },
    Lesson {
        item: LessonsListItem,
    },
}

// == Fetch lesson content blocks ==

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ContentBlocksResponse {
    pub data: ContentBlocksData,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ContentBlocksData {
    pub content_blocks: Vec<ContentBlock>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ContentBlock {
    // id: Id,
    pub children: Vec<ContentBlock>,
    pub content: Content,
    pub goods: Option<Vec<Good>>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Content {
    pub text: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Good {
    pub digital: DigitalGood,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct DigitalGood {
    // id: Id,
    pub wistia_data: Option<WistiaData>,
    pub file: Option<FileAsset>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct WistiaData {
    // id: Id,
    pub name: Option<String>,
    pub r#type: Option<String>,
    pub assets: Option<Vec<Asset>>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Asset {
    pub url: String,
    #[serde(rename = "fileSize")]
    pub file_size: usize,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct FileAsset {
    pub name: Option<String>,
    pub original: Option<String>,
}
