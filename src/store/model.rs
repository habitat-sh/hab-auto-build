use chrono::NaiveDateTime;
use diesel::{Insertable, Queryable};

#[derive(Debug, Queryable)]
pub struct FileModificationRecord {
    pub plan_context_path: String,
    pub file_path: String,
    pub real_modified_at: NaiveDateTime,
    pub alternate_modified_at: NaiveDateTime,
}

#[derive(Debug, Queryable)]
pub struct ArtifactContextRecord {
    pub hash: String,
    pub context: String,
}


#[derive(Debug, Queryable)]
pub struct SourceContextRecord {
    pub hash: String,
    pub context: String,
}

