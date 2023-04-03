mod artifact;
mod source;

use std::{
    collections::{HashMap, HashSet},
    fmt::Display,
};

use crate::core::{ArtifactCache, ArtifactContext, PackageIdent, PlanContext, SourceContext};

use color_eyre::eyre::{eyre, Result};
use owo_colors::OwoColorize;
use serde::{Deserialize, Serialize};
use toml_edit::{Array, Document, Formatted, InlineTable, Value};
use tracing::debug;

use self::{
    artifact::elf::{ElfCheck, ElfRule},
    artifact::package::{PackageBeforeCheck, PackageRule},
    artifact::{
        elf::ElfRuleOptions,
        package::{PackageAfterCheck, PackageRuleOptions},
        script::{ScriptCheck, ScriptRule, ScriptRuleOptions},
    },
    source::license::{
        LicenseCheck, LicenseRule, LicenseRuleOptions,
    },
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
pub(crate) struct ContextRules {
    #[serde(default)]
    source_rules: Vec<SourceRule>,
    #[serde(default)]
    artifact_rules: Vec<ArtifactRule>,
}

impl ContextRules {
    pub fn merge(mut self, other: &ContextRules) -> ContextRules {
        self.source_rules.extend_from_slice(&other.source_rules);
        self.artifact_rules.extend_from_slice(&other.artifact_rules);
        self
    }
    pub fn from_str(value: &str) -> Result<ContextRules> {
        let document = value.parse::<Document>()?;
        let mut restructured_document = Document::new();
        let mut restructured_rules = Array::default();
        if let Some(rules) = document.get("rules") {
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
        let plan_config: PlanConfig = toml_edit::de::from_document(restructured_document)
            .map_err(|err| eyre!("Invalid .hab-plan-config.toml file: {}", err))?;
        let mut context_rules = ContextRules {
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

impl Default for ContextRules {
    fn default() -> Self {
        Self {
            source_rules: vec![
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
            ],
            artifact_rules: vec![
                ArtifactRule {
                    options: ArtifactRuleOptions::Elf(ElfRuleOptions::MissingRPathEntryDependency(
                        Default::default(),
                    )),
                },
                ArtifactRule {
                    options: ArtifactRuleOptions::Elf(ElfRuleOptions::BadRPathEntry(
                        Default::default(),
                    )),
                },
                ArtifactRule {
                    options: ArtifactRuleOptions::Elf(
                        ElfRuleOptions::MissingRunPathEntryDependency(Default::default()),
                    ),
                },
                ArtifactRule {
                    options: ArtifactRuleOptions::Elf(ElfRuleOptions::BadRunPathEntry(
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
                    options: ArtifactRuleOptions::Elf(
                        ElfRuleOptions::MissingELFInterpreterDependency(Default::default()),
                    ),
                },
                ArtifactRule {
                    options: ArtifactRuleOptions::Elf(ElfRuleOptions::UnexpectedELFInterpreter(
                        Default::default(),
                    )),
                },
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
                    options: ArtifactRuleOptions::Package(
                        PackageRuleOptions::EmptyTopLevelDirectory(Default::default()),
                    ),
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
                    options: ArtifactRuleOptions::Package(
                        PackageRuleOptions::DuplicateRuntimeBinary(Default::default()),
                    ),
                },
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
                    options: ArtifactRuleOptions::Script(
                        ScriptRuleOptions::ScriptInterpreterNotFound(Default::default()),
                    ),
                },
                ArtifactRule {
                    options: ArtifactRuleOptions::Script(
                        ScriptRuleOptions::UnlistedScriptInterpreter(Default::default()),
                    ),
                },
                ArtifactRule {
                    options: ArtifactRuleOptions::Script(
                        ScriptRuleOptions::MissingScriptInterpreterDependency(Default::default()),
                    ),
                },
            ],
        }
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
    Elf(ElfRuleOptions),
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
    #[serde(rename = "elf")]
    Elf(ElfRule),
    #[serde(rename = "package")]
    Package(PackageRule),
    #[serde(rename = "script")]
    Script(ScriptRule),
}

impl Display for ArtifactCheckViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ArtifactCheckViolation::Elf(rule) => write!(f, "{}", rule),
            ArtifactCheckViolation::Package(rule) => write!(f, "{}", rule),
            ArtifactCheckViolation::Script(rule) => write!(f, "{}", rule),
        }
    }
}

pub(crate) trait SourceCheck {
    fn source_context_check_with_plan(
        &self,
        rules: &ContextRules,
        plan_context: &PlanContext,
        source_context: &SourceContext,
    ) -> Vec<LeveledSourceCheckViolation>;
    fn source_context_check_with_artifact(
        &self,
        rules: &ContextRules,
        artifact_context: &ArtifactContext,
        source_context: &SourceContext,
    ) -> Vec<LeveledSourceCheckViolation>;
}

pub(crate) trait ArtifactCheck {
    fn artifact_context_check(
        &self,
        rules: &ContextRules,
        checker_context: &mut CheckerContext,
        artifact_cache: &ArtifactCache,
        artifact_context: &ArtifactContext,
    ) -> Vec<LeveledArtifactCheckViolation>;
}

#[derive(Debug, Default)]
pub(crate) struct CheckerContext {
    tdeps: Option<HashMap<PackageIdent, ArtifactContext>>,
    runtime_artifacts: Option<Vec<ArtifactContext>>,
    unused_deps: Option<HashSet<PackageIdent>>,
}

impl CheckerContext {
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
    pub fn new() -> Checker {
        Checker {
            source_checks: vec![Box::new(LicenseCheck::default())],
            artifact_checks: vec![
                Box::new(PackageBeforeCheck::default()),
                Box::new(ElfCheck::default()),
                Box::new(ScriptCheck::default()),
                Box::new(PackageAfterCheck::default()),
            ],
        }
    }
}

impl SourceCheck for Checker {
    fn source_context_check_with_plan(
        &self,
        rules: &ContextRules,
        plan_context: &PlanContext,
        source_context: &SourceContext,
    ) -> Vec<LeveledSourceCheckViolation> {
        debug!("Checking package source against plan for issues");
        let mut violations = Vec::new();
        for source_check in self.source_checks.iter() {
            let mut source_violations =
                source_check.source_context_check_with_plan(rules, plan_context, source_context);
            violations.append(&mut source_violations);
        }
        violations
    }

    fn source_context_check_with_artifact(
        &self,
        rules: &ContextRules,
        artifact_context: &ArtifactContext,
        source_context: &SourceContext,
    ) -> Vec<LeveledSourceCheckViolation> {
        debug!("Checking package source against artifact for issues");
        let mut violations = Vec::new();
        for source_check in self.source_checks.iter() {
            let mut source_violations = source_check.source_context_check_with_artifact(
                rules,
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
        rules: &ContextRules,
        checker_context: &mut CheckerContext,
        artifact_cache: &ArtifactCache,
        artifact_context: &ArtifactContext,
    ) -> Vec<LeveledArtifactCheckViolation> {
        debug!("Checking package artifact for issues");
        let mut violations = Vec::new();
        for artifact_check in self.artifact_checks.iter() {
            let mut artifact_violations = artifact_check.artifact_context_check(
                rules,
                checker_context,
                artifact_cache,
                artifact_context,
            );
            violations.append(&mut artifact_violations);
        }
        violations
    }
}
