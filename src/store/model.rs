use diesel::Queryable;

#[derive(Debug, Queryable)]
pub struct FileModificationRecord {
    pub plan_context_path: String,
    pub file_path: String,
    pub real_modified_at: String,
    pub alternate_modified_at: String,
}

#[derive(Debug, Queryable)]
pub struct BuildTimeRecord {
    #[allow(dead_code)]
    pub build_ident: String,
    pub duration_in_secs: i32,
}

#[derive(Debug, Queryable)]
pub struct ArtifactContextRecord {
    #[allow(dead_code)]
    pub hash: String,
    pub context: String,
}

#[derive(Debug, Queryable)]
pub struct SourceContextRecord {
    #[allow(dead_code)]
    pub hash: String,
    pub context: String,
}
