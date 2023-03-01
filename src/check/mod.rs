mod artifact;
mod source;

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::core::{ArtifactCache, ArtifactContext, PackageIdent, PlanContext, SourceContext};

use self::{
    artifact::elf::{ElfCheck, ElfRule},
    artifact::package::{PackageCheck, PackageRule},
    artifact::{
        elf::ElfRuleOptions,
        package::PackageRuleOptions,
        script::{ScriptCheck, ScriptRule, ScriptRuleOptions},
    },
    source::license::{
        LicenseCheck, LicenseNotFound, LicenseNotFoundOptions, LicenseRule, LicenseRuleOptions,
        MissingLicenseOptions,
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

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "category")]
pub(crate) enum SourceCheckViolation {
    #[serde(rename = "license")]
    License(LicenseRule),
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct LeveledArtifactCheckViolation {
    pub level: ViolationLevel,
    pub violation: ArtifactCheckViolation,
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
                Box::new(PackageCheck::default()),
                Box::new(ElfCheck::default()),
                Box::new(ScriptCheck::default()),
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
