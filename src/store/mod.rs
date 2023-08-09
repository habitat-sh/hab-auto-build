pub mod model;
pub mod schema;

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use crate::{
    core::{
        ArtifactContext, Blake3, GitRepo, InnerArtifactContext, PackageBuildIdent,
        PackageSha256Sum, PackageSource, PlanContextPath, SourceContext,
    },
    store::model::SourceContextRecord,
};

use self::model::{ArtifactContextRecord, BuildTimeRecord, FileModificationRecord};
use chrono::{DateTime, NaiveDateTime, Utc};
use color_eyre::eyre::{Context, Result};

use diesel::{
    delete, insert_into,
    prelude::*,
    r2d2::{ConnectionManager, Pool, PooledConnection},
    update,
};
use diesel_migrations::{embed_migrations, EmbeddedMigrations, MigrationHarness};
use lazy_static::__Deref;
use tempdir::TempDir;
use tracing::{debug, trace};

pub const MIGRATIONS: EmbeddedMigrations = embed_migrations!("migrations");
pub const TIMESTAMP_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%.9f";

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub(crate) struct StorePath(PathBuf);

impl AsRef<Path> for StorePath {
    fn as_ref(&self) -> &Path {
        self.0.as_path()
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub(crate) struct InvalidPackageSourceStorePath(PathBuf);

impl AsRef<Path> for InvalidPackageSourceStorePath {
    fn as_ref(&self) -> &Path {
        self.0.as_path()
    }
}

impl InvalidPackageSourceStorePath {
    pub fn archive_data_path(&self) -> InvalidPackageSourceArchiveStorePath {
        InvalidPackageSourceArchiveStorePath(self.0.join("source"))
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub(crate) struct TempDirStorePath(PathBuf);

impl AsRef<Path> for TempDirStorePath {
    fn as_ref(&self) -> &Path {
        self.0.as_path()
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub(crate) struct PackageBuildArtifactsStorePath(PathBuf);

impl AsRef<Path> for PackageBuildArtifactsStorePath {
    fn as_ref(&self) -> &Path {
        self.0.as_path()
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub(crate) struct PackageBuildSuccessLogsStorePath(PathBuf);

impl AsRef<Path> for PackageBuildSuccessLogsStorePath {
    fn as_ref(&self) -> &Path {
        self.0.as_path()
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub(crate) struct PackageBuildFailureLogsStorePath(PathBuf);

impl AsRef<Path> for PackageBuildFailureLogsStorePath {
    fn as_ref(&self) -> &Path {
        self.0.as_path()
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub(crate) struct PackageSourceStorePath(PathBuf);

impl AsRef<Path> for PackageSourceStorePath {
    fn as_ref(&self) -> &Path {
        self.0.as_path()
    }
}

impl PackageSourceStorePath {
    pub fn archive_data_path(&self) -> PackageSourceArchiveStorePath {
        PackageSourceArchiveStorePath(self.0.join("source"))
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub(crate) struct PackageSourceArchiveStorePath(PathBuf);

impl AsRef<Path> for PackageSourceArchiveStorePath {
    fn as_ref(&self) -> &Path {
        self.0.as_path()
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub(crate) struct InvalidPackageSourceArchiveStorePath(PathBuf);

impl AsRef<Path> for InvalidPackageSourceArchiveStorePath {
    fn as_ref(&self) -> &Path {
        self.0.as_path()
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub(crate) struct PackageSourceLicenseStorePath(PathBuf);

impl AsRef<Path> for PackageSourceLicenseStorePath {
    fn as_ref(&self) -> &Path {
        self.0.as_path()
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub(crate) struct RepoSourceGitDirectoryPath(PathBuf);

impl AsRef<Path> for RepoSourceGitDirectoryPath {
    fn as_ref(&self) -> &Path {
        self.0.as_path()
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub(crate) struct RepoSourceGitWorkTreePath(PathBuf);

impl AsRef<Path> for RepoSourceGitWorkTreePath {
    fn as_ref(&self) -> &Path {
        self.0.as_path()
    }
}

#[derive(Clone)]
pub(crate) struct Store {
    path: StorePath,
    pool: Pool<ConnectionManager<SqliteConnection>>,
}

impl Store {
    pub fn new(path: impl AsRef<Path>) -> Result<Store> {
        std::fs::create_dir_all(path.as_ref())?;
        let db_url = path
            .as_ref()
            .join("hab-auto-build.sqlite")
            .to_str()
            .unwrap()
            .to_string();

        let manager = ConnectionManager::<SqliteConnection>::new(db_url);
        // Refer to the `r2d2` documentation for more methods to use
        // when building a connection pool
        let pool = Pool::builder()
            .test_on_check_out(true)
            .build(manager)?;
        let mut connection = pool.get()?;
        connection
            .run_pending_migrations(MIGRATIONS)
            .expect("Failed to run migration");
        Ok(Store {
            path: StorePath(path.as_ref().to_path_buf()),
            pool,
        })
    }

    pub fn temp_dir_path(&self) -> TempDirStorePath {
        TempDirStorePath(self.path.as_ref().join("tmp"))
    }

    pub fn temp_dir(&self, prefix: &str) -> Result<TempDir> {
        let tmp_parent_dir = self.path.as_ref().join("tmp");
        std::fs::create_dir_all(tmp_parent_dir.as_path())?;
        TempDir::new_in(tmp_parent_dir, prefix).with_context(|| {
            format!(
                "Failed to create temporary directory in hab-auto-build store at '{}'",
                self.path.as_ref().join("tmp").display()
            )
        })
    }

    pub fn get_connection(&self) -> Result<PooledConnection<ConnectionManager<SqliteConnection>>> {
        trace!("Opening database connection");
        Ok(self.pool.get()?)
    }

    pub fn package_build_artifacts_path(&self) -> PackageBuildArtifactsStorePath {
        PackageBuildArtifactsStorePath(self.path.as_ref().join("artifacts"))
    }
    pub fn package_build_success_logs_path(&self) -> PackageBuildSuccessLogsStorePath {
        PackageBuildSuccessLogsStorePath(self.path.as_ref().join("build-success-logs"))
    }
    pub fn package_build_failure_logs_path(&self) -> PackageBuildFailureLogsStorePath {
        PackageBuildFailureLogsStorePath(self.path.as_ref().join("build-failure-logs"))
    }
    pub fn package_source_store_path(&self, source: &PackageSource) -> PackageSourceStorePath {
        PackageSourceStorePath(
            self.path
                .as_ref()
                .join("sources")
                .join(source.shasum.to_string()),
        )
    }
    pub fn invalid_source_store_path(
        &self,
        source: &PackageSource,
    ) -> InvalidPackageSourceStorePath {
        InvalidPackageSourceStorePath(
            self.path
                .as_ref()
                .join("invalid-sources")
                .join(source.shasum.to_string()),
        )
    }
    pub fn repo_git_directory_path(&self, source: &GitRepo) -> RepoSourceGitDirectoryPath {
        RepoSourceGitDirectoryPath(
            self.path.as_ref().join("git-repos").join(
                Blake3::hash_value(format!("{}", source.url))
                    .unwrap()
                    .to_string(),
            ),
        )
    }
    pub fn repo_git_work_tree_path(&self, source: &GitRepo) -> RepoSourceGitWorkTreePath {
        RepoSourceGitWorkTreePath(
            self.path.as_ref().join("git-trees").join(
                Blake3::hash_value(format!("{}#{}", source.url, source.commit))
                    .unwrap()
                    .to_string(),
            ),
        )
    }
}

pub(crate) struct ModificationIndex(
    HashMap<PathBuf, HashMap<PathBuf, (DateTime<Utc>, DateTime<Utc>)>>,
);

impl ModificationIndex {
    pub(crate) fn file_alternate_modified_at_get(
        &self,
        plan_context_path_value: &PlanContextPath,
        file_path_value: impl AsRef<Path>,
        real_modified_at_value: DateTime<Utc>,
    ) -> Option<DateTime<Utc>> {
        self.0
            .get(plan_context_path_value.as_ref())
            .and_then(|m| m.get(file_path_value.as_ref()))
            .and_then(|(real_modified_at, alternate_modified_at)| {
                if *real_modified_at == real_modified_at_value {
                    Some(*alternate_modified_at)
                } else {
                    None
                }
            })
    }
}

pub(crate) fn files_alternate_modified_at_get_full_index(
    connection: &mut SqliteConnection,
) -> Result<ModificationIndex> {
    use crate::store::schema::file_modifications::dsl::*;
    let mut results: HashMap<PathBuf, HashMap<PathBuf, (DateTime<Utc>, DateTime<Utc>)>> =
        HashMap::new();
    let rows = file_modifications.load::<FileModificationRecord>(connection)?;
    for row in rows {
        results
            .entry(PathBuf::from(row.plan_context_path))
            .or_default()
            .entry(PathBuf::from(row.file_path))
            .or_insert((
                DateTime::<Utc>::from_utc(
                    NaiveDateTime::parse_from_str(&row.real_modified_at, TIMESTAMP_FORMAT).unwrap(),
                    Utc,
                ),
                DateTime::<Utc>::from_utc(
                    NaiveDateTime::parse_from_str(&row.alternate_modified_at, TIMESTAMP_FORMAT)
                        .unwrap(),
                    Utc,
                ),
            ));
    }
    Ok(ModificationIndex(results))
}

pub(crate) fn build_time_get(
    connection: &mut SqliteConnection,
    build_ident_value: &PackageBuildIdent,
) -> Result<Option<BuildTimeRecord>> {
    use crate::store::schema::build_times::dsl::*;
    Ok(build_times
        .filter(build_ident.eq(build_ident_value.to_string()))
        .load::<BuildTimeRecord>(connection)?
        .pop())
}

pub(crate) fn build_time_put(
    connection: &mut SqliteConnection,
    build_ident_value: &PackageBuildIdent,
    build_duration_in_secs_value: i32,
) -> Result<()> {
    use crate::store::schema::build_times::dsl::*;
    if build_times
        .filter(build_ident.eq(build_ident_value.to_string()))
        .load::<BuildTimeRecord>(connection)?
        .first()
        .is_none()
    {
        insert_into(build_times)
            .values((
                build_ident.eq(build_ident_value.to_string()),
                duration_in_secs.eq(build_duration_in_secs_value),
            ))
            .execute(connection)?;
    } else {
        update(build_times.filter(build_ident.eq(build_ident_value.to_string())))
            .set(duration_in_secs.eq(build_duration_in_secs_value))
            .execute(connection)?;
    }
    Ok(())
}

pub(crate) fn source_context_get(
    connection: &mut SqliteConnection,
    hash_value: &PackageSha256Sum,
) -> Result<Option<SourceContext>> {
    use crate::store::schema::source_contexts::dsl::*;
    if let Some(row) = source_contexts
        .filter(hash.eq(hash_value.to_string()))
        .load::<SourceContextRecord>(connection)?
        .first()
    {
        Ok(Some(serde_json::from_str(&row.context)?))
    } else {
        Ok(None)
    }
}

pub(crate) fn source_context_put(
    connection: &mut SqliteConnection,
    hash_value: &PackageSha256Sum,
    source_context_value: &SourceContext,
) -> Result<()> {
    use crate::store::schema::source_contexts::dsl::*;
    if source_contexts
        .filter(hash.eq(hash_value.to_string()))
        .load::<SourceContextRecord>(connection)?
        .first()
        .is_none()
    {
        insert_into(source_contexts)
            .values((
                hash.eq(hash_value.to_string()),
                context.eq(serde_json::to_string(source_context_value)?),
            ))
            .execute(connection)?;
    }
    Ok(())
}

pub(crate) fn artifact_context_get(
    connection: &mut SqliteConnection,
    hash_value: &Blake3,
) -> Result<Option<ArtifactContext>> {
    use crate::store::schema::artifact_contexts::dsl::*;
    if let Some(row) = artifact_contexts
        .filter(hash.eq(hash_value.to_string()))
        .load::<ArtifactContextRecord>(connection)?
        .first()
    {
        Ok(Some(
            serde_json::from_str::<InnerArtifactContext>(&row.context)?.into(),
        ))
    } else {
        Ok(None)
    }
}

pub(crate) fn artifact_context_put(
    connection: &mut SqliteConnection,
    hash_value: &Blake3,
    artifact_context_value: &ArtifactContext,
) -> Result<()> {
    use crate::store::schema::artifact_contexts::dsl::*;
    insert_into(artifact_contexts)
        .values((
            hash.eq(hash_value.to_string()),
            context.eq(serde_json::to_string(artifact_context_value.deref())?),
        ))
        .execute(connection)?;
    Ok(())
}

pub(crate) fn file_alternate_modified_at_get(
    connection: &mut SqliteConnection,
    plan_context_path_value: &PlanContextPath,
    file_path_value: impl AsRef<Path>,
    real_modified_at_value: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>> {
    use crate::store::schema::file_modifications::dsl::*;
    if let Some(row) = file_modifications
        .filter(plan_context_path.eq(plan_context_path_value.as_ref().to_str().unwrap()))
        .filter(file_path.eq(file_path_value.as_ref().to_str().unwrap()))
        .filter(
            real_modified_at.eq(real_modified_at_value
                .naive_utc()
                .format(TIMESTAMP_FORMAT)
                .to_string()
                .as_str()),
        )
        .limit(1)
        .load::<FileModificationRecord>(connection)?
        .first()
    {
        Ok(Some(DateTime::<Utc>::from_utc(
            NaiveDateTime::parse_from_str(&row.alternate_modified_at, TIMESTAMP_FORMAT).unwrap(),
            Utc,
        )))
    } else {
        Ok(None)
    }
}
pub(crate) fn file_alternate_modified_at_put(
    connection: &mut SqliteConnection,
    plan_context_path_value: &PlanContextPath,
    file_path_value: impl AsRef<Path>,
    real_modified_at_value: DateTime<Utc>,
    alternate_modified_at_value: DateTime<Utc>,
) -> Result<()> {
    use crate::store::schema::file_modifications::dsl::*;
    insert_into(file_modifications)
        .values((
            plan_context_path.eq(plan_context_path_value.as_ref().to_str().unwrap()),
            file_path.eq(file_path_value.as_ref().to_str().unwrap()),
            real_modified_at.eq(&real_modified_at_value
                .naive_utc()
                .format(TIMESTAMP_FORMAT)
                .to_string()),
            alternate_modified_at.eq(&alternate_modified_at_value
                .naive_utc()
                .format(TIMESTAMP_FORMAT)
                .to_string()),
        ))
        .execute(connection)?;
    Ok(())
}
pub(crate) fn plan_context_alternate_modified_at_delete(
    connection: &mut SqliteConnection,
    plan_context_path_value: &PlanContextPath,
) -> Result<Option<HashMap<PathBuf, (DateTime<Utc>, DateTime<Utc>)>>> {
    use crate::store::schema::file_modifications::dsl::*;
    let existing_file_modifications = file_modifications
        .filter(plan_context_path.eq(plan_context_path_value.as_ref().to_str().unwrap()))
        .limit(1)
        .load::<FileModificationRecord>(connection)?;
    let results: HashMap<PathBuf, (DateTime<Utc>, DateTime<Utc>)> = existing_file_modifications
        .into_iter()
        .map(|row| {
            (
                PathBuf::from(row.file_path),
                (
                    DateTime::<Utc>::from_utc(
                        NaiveDateTime::parse_from_str(&row.real_modified_at, TIMESTAMP_FORMAT)
                            .unwrap(),
                        Utc,
                    ),
                    DateTime::<Utc>::from_utc(
                        NaiveDateTime::parse_from_str(&row.alternate_modified_at, TIMESTAMP_FORMAT)
                            .unwrap(),
                        Utc,
                    ),
                ),
            )
        })
        .collect();
    delete(
        file_modifications
            .filter(plan_context_path.eq(plan_context_path_value.as_ref().to_str().unwrap())),
    )
    .execute(connection)?;
    if results.is_empty() {
        Ok(None)
    } else {
        Ok(Some(results))
    }
}
