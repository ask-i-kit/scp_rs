#[derive(Debug, Clone, PartialEq)]
pub struct FileEntry {
    pub perm: String,
    pub size: u64,
    pub date: String,
    pub name: String,
}

#[derive(PartialEq, Clone, Copy)]
pub enum SortColumn {
    None,
    Permission,
    Size,
    Date,
    Name,
}

#[derive(PartialEq, Clone, Copy)]
pub enum SortDirection {
    Asc,
    Desc,
}

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FavoriteConnection {
    pub name: String,
    pub host: String,
    pub user: String,
    // Saving password for convenience as per user request (even if insecure)
    pub password: String, 
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DirectoryBookmark {
    pub name: String,
    pub path: String,
    pub host: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileEncoding {
    Utf8,
    ShiftJis,
}

impl std::fmt::Display for FileEncoding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FileEncoding::Utf8 => write!(f, "UTF-8"),
            FileEncoding::ShiftJis => write!(f, "Shift-JIS"),
        }
    }
}

