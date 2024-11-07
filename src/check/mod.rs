mod artifact;
mod source;

use std::{
    collections::{HashMap, HashSet},
    fmt::Display,
};

use crate::{
    core::{ArtifactCache, ArtifactContext, PackageIdent, PlanContext, SourceContext},
    store::Store,
};

#[cfg(not(target_os = "windows"))]
use crate::core::PackageTarget;

#[cfg(not(target_os = "windows"))]
use color_eyre::{
    eyre::{eyre, Result},
    Help, SectionExt,
};

use owo_colors::OwoColorize;
use serde::{Deserialize, Serialize};

#[cfg(not(target_os = "windows"))]
use toml_edit::{Array, DocumentMut, Formatted, InlineTable, Value};

use tracing::debug;

#[cfg(target_os = "linux")]
use self::artifact::elf::{ElfCheck, ElfRule, ElfRuleOptions};

#[cfg(target_os = "macos")]
use self::artifact::macho::{MachORule, MachORuleOptions};

use self::{
    artifact::package::{PackageBeforeCheck, PackageRule},
    artifact::{
        package::{PackageAfterCheck, PackageRuleOptions},
        script::{ScriptCheck, ScriptRule, ScriptRuleOptions},
    },
    source::license::{LicenseCheck, LicenseRule, LicenseRuleOptions},
};

#[derive(Debug, Serialize, Deserialize, Copy, Clone, PartialEq, Eq)]
pub(crate) enum ViolationLevel {
    #[serde(rename = "warn")]
    Warn,
    #[serde(rename = "error")]
    Error,
    #[serde(rename = "off")]
    Off,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct PlanConfig {
    #[serde(default)]
    rules: Vec<RuleConfig>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(untagged)]
pub(crate) enum RuleConfig {
    Source(SourceRule),
    Artifact(ArtifactRule),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct PlanContextConfig {
    #[serde(default, rename = "docker-image")]
    pub docker_image: Option<String>,
    pub sandbox: Option<bool>,
    #[serde(default)]
    pub source_rules: Vec<SourceRule>,
    #[serde(default)]
    pub artifact_rules: Vec<ArtifactRule>,
}

impl PlanContextConfig {
    pub fn merge(mut self, other: &PlanContextConfig) -> PlanContextConfig {
        self.source_rules.extend_from_slice(&other.source_rules);
        self.artifact_rules.extend_from_slice(&other.artifact_rules);
        self
    }

    #[cfg(not(target_os = "windows"))]
    pub fn from_str(value: &str, target: PackageTarget) -> Result<PlanContextConfig> {
        let document = value.parse::<DocumentMut>()?;
        let mut restructured_document = DocumentMut::new();
        let mut restructured_rules = Array::default();
        let rule_sets = [
            document.get("rules"),
            document
                .get(target.to_string().as_str())
                .and_then(|v| v.get("rules")),
        ];
        for rules in rule_sets.into_iter().flatten() {
            let rules = rules
                .as_table()
                .ok_or(eyre!("Invalid plan configuration, 'rules' must be a table"))?;
            for (rule_id, rule_config) in rules.iter() {
                if let Some(level) = rule_config.as_str() {
                    let mut rule = InlineTable::default();
                    rule.insert(
                        "id",
                        Value::String(Formatted::<String>::new(rule_id.to_string())),
                    );
                    let mut rule_options = InlineTable::default();
                    rule_options.insert(
                        "level",
                        Value::String(Formatted::<String>::new(level.to_string())),
                    );
                    rule.insert("options", Value::InlineTable(rule_options));
                    restructured_rules.push(Value::InlineTable(rule));
                } else if rule_config.is_inline_table() {
                    let mut rule = InlineTable::default();
                    rule.insert(
                        "id",
                        Value::String(Formatted::<String>::new(rule_id.to_string())),
                    );
                    rule.insert(
                        "options",
                        Value::InlineTable(rule_config.as_inline_table().unwrap().clone()),
                    );
                    restructured_rules.push(Value::InlineTable(rule));
                } else {
                    return Err(eyre!(
                        "Invalid rule configuration for '{}'",
                        rule_id.to_string()
                    ));
                }
            }
        }

        restructured_document.insert("rules", toml_edit::value(restructured_rules));
        let plan_config: PlanConfig = toml_edit::de::from_document(restructured_document.clone())
            .map_err(|err| eyre!("Invalid .hab-plan-config.toml file: {}", err))
            .with_section(|| {
                restructured_document
                    .to_string()
                    .header("Restructured Rules:")
            })?;
        let mut context_rules = PlanContextConfig {
            sandbox: document.get("sandbox").and_then(|value| value.as_bool()),
            docker_image: document
                .get("docker-image")
                .map(|value| {
                    value
                        .as_str()
                        .ok_or(eyre!("Invalid docker image name"))
                        .map(String::from)
                })
                .transpose()?,
            source_rules: vec![],
            artifact_rules: vec![],
        };
        for rule in plan_config.rules {
            match rule {
                RuleConfig::Source(source_rule) => context_rules.source_rules.push(source_rule),
                RuleConfig::Artifact(artifact_rule) => {
                    context_rules.artifact_rules.push(artifact_rule)
                }
            }
        }
        Ok(context_rules)
    }
}

impl Default for PlanContextConfig {
    fn default() -> Self {
        let mut license_rules = vec![
            SourceRule {
                options: SourceRuleOptions::License(LicenseRuleOptions::MissingLicense(
                    Default::default(),
                )),
            },
            SourceRule {
                options: SourceRuleOptions::License(LicenseRuleOptions::LicenseNotFound(
                    Default::default(),
                )),
            },
            SourceRule {
                options: SourceRuleOptions::License(LicenseRuleOptions::InvalidLicenseExpression(
                    Default::default(),
                )),
            },
        ];
        #[cfg(target_os = "linux")]
        let mut elf_rules = vec![
            ArtifactRule {
                options: ArtifactRuleOptions::Elf(ElfRuleOptions::MissingRPathEntryDependency(
                    Default::default(),
                )),
            },
            ArtifactRule {
                options: ArtifactRuleOptions::Elf(
                    ElfRuleOptions::BadRPathEntry(Default::default()),
                ),
            },
            ArtifactRule {
                options: ArtifactRuleOptions::Elf(ElfRuleOptions::UnusedRPathEntry(
                    Default::default(),
                )),
            },
            ArtifactRule {
                options: ArtifactRuleOptions::Elf(ElfRuleOptions::MissingRunPathEntryDependency(
                    Default::default(),
                )),
            },
            ArtifactRule {
                options: ArtifactRuleOptions::Elf(ElfRuleOptions::BadRunPathEntry(
                    Default::default(),
                )),
            },
            ArtifactRule {
                options: ArtifactRuleOptions::Elf(ElfRuleOptions::UnusedRunPathEntry(
                    Default::default(),
                )),
            },
            ArtifactRule {
                options: ArtifactRuleOptions::Elf(ElfRuleOptions::LibraryDependencyNotFound(
                    Default::default(),
                )),
            },
            ArtifactRule {
                options: ArtifactRuleOptions::Elf(ElfRuleOptions::BadLibraryDependency(
                    Default::default(),
                )),
            },
            ArtifactRule {
                options: ArtifactRuleOptions::Elf(ElfRuleOptions::BadELFInterpreter(
                    Default::default(),
                )),
            },
            ArtifactRule {
                options: ArtifactRuleOptions::Elf(ElfRuleOptions::HostELFInterpreter(
                    Default::default(),
                )),
            },
            ArtifactRule {
                options: ArtifactRuleOptions::Elf(ElfRuleOptions::ELFInterpreterNotFound(
                    Default::default(),
                )),
            },
            ArtifactRule {
                options: ArtifactRuleOptions::Elf(ElfRuleOptions::MissingELFInterpreterDependency(
                    Default::default(),
                )),
            },
            ArtifactRule {
                options: ArtifactRuleOptions::Elf(ElfRuleOptions::UnexpectedELFInterpreter(
                    Default::default(),
                )),
            },
        ];
        #[cfg(target_os = "macos")]
        let mut macho_rules = vec![
            ArtifactRule {
                options: ArtifactRuleOptions::MachO(MachORuleOptions::MissingRPathEntryDependency(
                    Default::default(),
                )),
            },
            ArtifactRule {
                options: ArtifactRuleOptions::MachO(MachORuleOptions::BadRPathEntry(
                    Default::default(),
                )),
            },
            ArtifactRule {
                options: ArtifactRuleOptions::MachO(MachORuleOptions::UnusedRPathEntry(
                    Default::default(),
                )),
            },
            ArtifactRule {
                options: ArtifactRuleOptions::MachO(MachORuleOptions::MissingLibraryDependency(
                    Default::default(),
                )),
            },
            ArtifactRule {
                options: ArtifactRuleOptions::MachO(MachORuleOptions::LibraryDependencyNotFound(
                    Default::default(),
                )),
            },
            ArtifactRule {
                options: ArtifactRuleOptions::MachO(MachORuleOptions::BadLibraryDependency(
                    Default::default(),
                )),
            },
        ];
        let mut package_rules = vec![
            ArtifactRule {
                options: ArtifactRuleOptions::Package(PackageRuleOptions::BadRuntimePathEntry(
                    Default::default(),
                )),
            },
            ArtifactRule {
                options: ArtifactRuleOptions::Package(
                    PackageRuleOptions::MissingRuntimePathEntryDependency(Default::default()),
                ),
            },
            ArtifactRule {
                options: ArtifactRuleOptions::Package(
                    PackageRuleOptions::MissingDependencyArtifact(Default::default()),
                ),
            },
            ArtifactRule {
                options: ArtifactRuleOptions::Package(PackageRuleOptions::DuplicateDependency(
                    Default::default(),
                )),
            },
            ArtifactRule {
                options: ArtifactRuleOptions::Package(PackageRuleOptions::EmptyTopLevelDirectory(
                    Default::default(),
                )),
            },
            ArtifactRule {
                options: ArtifactRuleOptions::Package(PackageRuleOptions::BrokenLink(
                    Default::default(),
                )),
            },
            ArtifactRule {
                options: ArtifactRuleOptions::Package(PackageRuleOptions::UnusedDependency(
                    Default::default(),
                )),
            },
            ArtifactRule {
                options: ArtifactRuleOptions::Package(PackageRuleOptions::DuplicateRuntimeBinary(
                    Default::default(),
                )),
            },
        ];
        let mut script_rules = vec![
            ArtifactRule {
                options: ArtifactRuleOptions::Script(ScriptRuleOptions::HostScriptInterpreter(
                    Default::default(),
                )),
            },
            ArtifactRule {
                options: ArtifactRuleOptions::Script(
                    ScriptRuleOptions::MissingEnvScriptInterpreter(Default::default()),
                ),
            },
            ArtifactRule {
                options: ArtifactRuleOptions::Script(
                    ScriptRuleOptions::EnvScriptInterpreterNotFound(Default::default()),
                ),
            },
            ArtifactRule {
                options: ArtifactRuleOptions::Script(ScriptRuleOptions::ScriptInterpreterNotFound(
                    Default::default(),
                )),
            },
            ArtifactRule {
                options: ArtifactRuleOptions::Script(ScriptRuleOptions::UnlistedScriptInterpreter(
                    Default::default(),
                )),
            },
            ArtifactRule {
                options: ArtifactRuleOptions::Script(
                    ScriptRuleOptions::MissingScriptInterpreterDependency(Default::default()),
                ),
            },
        ];
        let mut config = Self {
            sandbox: None,
            docker_image: None,
            source_rules: vec![],
            artifact_rules: vec![],
        };
        config.source_rules.append(&mut license_rules);
        config.artifact_rules.append(&mut package_rules);
        config.artifact_rules.append(&mut script_rules);
        #[cfg(target_os = "linux")]
        config.artifact_rules.append(&mut elf_rules);
        #[cfg(target_os = "macos")]
        config.artifact_rules.append(&mut macho_rules);
        config
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct SourceRule {
    #[serde(flatten)]
    options: SourceRuleOptions,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(untagged)]
pub(crate) enum SourceRuleOptions {
    License(LicenseRuleOptions),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub(crate) struct ArtifactRule {
    #[serde(flatten)]
    options: ArtifactRuleOptions,
}
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(untagged)]
pub(crate) enum ArtifactRuleOptions {
    #[cfg(target_os = "linux")]
    Elf(ElfRuleOptions),
    #[cfg(target_os = "macos")]
    MachO(MachORuleOptions),
    Package(PackageRuleOptions),
    Script(ScriptRuleOptions),
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct LeveledSourceCheckViolation {
    pub level: ViolationLevel,
    pub violation: SourceCheckViolation,
}

impl Display for LeveledSourceCheckViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.level {
            ViolationLevel::Warn => write!(
                f,
                "{}{} {}",
                "warning: ".yellow().bold(),
                format!(
                    "[{}]",
                    serde_json::to_value(&self.violation).unwrap()["rule"]
                        .as_str()
                        .unwrap()
                )
                .bright_black(),
                self.violation,
            ),
            ViolationLevel::Error => write!(
                f,
                "{}{} {}",
                "  error: ".red().bold(),
                format!(
                    "[{}]",
                    serde_json::to_value(&self.violation).unwrap()["rule"]
                        .as_str()
                        .unwrap()
                )
                .bright_black(),
                self.violation,
            ),
            ViolationLevel::Off => write!(f, ""),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "category")]
pub(crate) enum SourceCheckViolation {
    #[serde(rename = "license")]
    License(LicenseRule),
}

impl Display for SourceCheckViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SourceCheckViolation::License(rule) => write!(f, "{}", rule),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct LeveledArtifactCheckViolation {
    pub level: ViolationLevel,
    pub violation: ArtifactCheckViolation,
}

impl Display for LeveledArtifactCheckViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.level {
            ViolationLevel::Warn => write!(
                f,
                "{}{} {}",
                "warning: ".yellow().bold(),
                format!(
                    "[{}]",
                    serde_json::to_value(&self.violation).unwrap()["rule"]
                        .as_str()
                        .unwrap()
                )
                .bright_black(),
                self.violation,
            ),
            ViolationLevel::Error => write!(
                f,
                "{}{} {}",
                "  error: ".red().bold(),
                format!(
                    "[{}]",
                    serde_json::to_value(&self.violation).unwrap()["rule"]
                        .as_str()
                        .unwrap()
                )
                .bright_black(),
                self.violation,
            ),
            ViolationLevel::Off => write!(f, ""),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "category")]
pub(crate) enum ArtifactCheckViolation {
    #[cfg(target_os = "linux")]
    #[serde(rename = "elf")]
    Elf(ElfRule),
    #[cfg(target_os = "macos")]
    #[serde(rename = "macho")]
    MachO(MachORule),
    #[serde(rename = "package")]
    Package(PackageRule),
    #[serde(rename = "script")]
    Script(ScriptRule),
}

impl Display for ArtifactCheckViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            #[cfg(target_os = "linux")]
            ArtifactCheckViolation::Elf(rule) => write!(f, "{}", rule),
            #[cfg(target_os = "macos")]
            ArtifactCheckViolation::MachO(rule) => write!(f, "{}", rule),
            ArtifactCheckViolation::Package(rule) => write!(f, "{}", rule),
            ArtifactCheckViolation::Script(rule) => write!(f, "{}", rule),
        }
    }
}

pub(crate) trait SourceCheck {
    fn source_context_check_with_plan(
        &self,
        plan_config: &PlanContextConfig,
        plan_context: &PlanContext,
        source_context: &SourceContext,
    ) -> Vec<LeveledSourceCheckViolation>;
    #[allow(dead_code)]
    fn source_context_check_with_artifact(
        &self,
        plan_config: &PlanContextConfig,
        artifact_context: &ArtifactContext,
        source_context: &SourceContext,
    ) -> Vec<LeveledSourceCheckViolation>;
}

pub(crate) trait ArtifactCheck {
    fn artifact_context_check(
        &self,
        store: &Store,
        plan_config: &PlanContextConfig,
        checker_context: &mut CheckerContext,
        artifact_cache: &mut ArtifactCache,
        artifact_context: &ArtifactContext,
    ) -> Vec<LeveledArtifactCheckViolation>;
}

#[derive(Debug, Default)]
pub(crate) struct CheckerContext {
    #[allow(dead_code)]
    tdeps: Option<HashMap<PackageIdent, ArtifactContext>>,
    #[allow(dead_code)]
    runtime_artifacts: Option<Vec<ArtifactContext>>,
    #[allow(dead_code)]
    unused_deps: Option<HashSet<PackageIdent>>,
}

impl CheckerContext {
    #[allow(dead_code)]
    pub fn mark_used(&mut self, dep: &PackageIdent) {
        if let Some(unused_deps) = self.unused_deps.as_mut() {
            unused_deps.remove(dep);
        }
    }
}

pub(crate) struct Checker {
    source_checks: Vec<Box<dyn SourceCheck>>,
    artifact_checks: Vec<Box<dyn ArtifactCheck>>,
}

impl Checker {
    #[cfg(target_os = "macos")]
    pub fn new() -> Checker {
        use self::artifact::macho::MachOCheck;

        Checker {
            source_checks: vec![Box::<LicenseCheck>::default()],
            artifact_checks: vec![
                Box::<PackageBeforeCheck>::default(),
                Box::<MachOCheck>::default(),
                Box::<ScriptCheck>::default(),
                Box::<PackageAfterCheck>::default(),
            ],
        }
    }
    #[cfg(target_os = "linux")]
    pub fn new() -> Checker {
        Checker {
            source_checks: vec![Box::<LicenseCheck>::default()],
            artifact_checks: vec![
                Box::<PackageBeforeCheck>::default(),
                Box::<ElfCheck>::default(),
                Box::<ScriptCheck>::default(),
                Box::<PackageAfterCheck>::default(),
            ],
        }
    }
    #[cfg(target_os = "windows")]
    pub fn new() -> Checker {
        use self::artifact::win::PeCheck;
        Checker {
            source_checks: vec![Box::<LicenseCheck>::default()],
            artifact_checks: vec![
                Box::<PackageBeforeCheck>::default(),
                Box::<PeCheck>::default(),
                Box::<ScriptCheck>::default(),
                Box::<PackageAfterCheck>::default(),
            ],
        }
    }
}

impl SourceCheck for Checker {
    fn source_context_check_with_plan(
        &self,
        plan_config: &PlanContextConfig,
        plan_context: &PlanContext,
        source_context: &SourceContext,
    ) -> Vec<LeveledSourceCheckViolation> {
        debug!("Checking package source against plan for issues");
        let mut violations = Vec::new();
        for source_check in self.source_checks.iter() {
            let mut source_violations = source_check.source_context_check_with_plan(
                plan_config,
                plan_context,
                source_context,
            );
            violations.append(&mut source_violations);
        }
        violations
    }

    fn source_context_check_with_artifact(
        &self,
        plan_config: &PlanContextConfig,
        artifact_context: &ArtifactContext,
        source_context: &SourceContext,
    ) -> Vec<LeveledSourceCheckViolation> {
        debug!("Checking package source against artifact for issues");
        let mut violations = Vec::new();
        for source_check in self.source_checks.iter() {
            let mut source_violations = source_check.source_context_check_with_artifact(
                plan_config,
                artifact_context,
                source_context,
            );
            violations.append(&mut source_violations);
        }
        violations
    }
}

impl ArtifactCheck for Checker {
    fn artifact_context_check(
        &self,
        store: &Store,
        plan_config: &PlanContextConfig,
        checker_context: &mut CheckerContext,
        artifact_cache: &mut ArtifactCache,
        artifact_context: &ArtifactContext,
    ) -> Vec<LeveledArtifactCheckViolation> {
        debug!("Checking package artifact for issues");
        let mut violations = Vec::new();
        for artifact_check in self.artifact_checks.iter() {
            let mut artifact_violations = artifact_check.artifact_context_check(
                store,
                plan_config,
                checker_context,
                artifact_cache,
                artifact_context,
            );
            violations.append(&mut artifact_violations);
        }
        violations
    }
}
