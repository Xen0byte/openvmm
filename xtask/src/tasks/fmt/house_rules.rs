// Copyright (C) Microsoft Corporation. All rights reserved.

use crate::fs_helpers::git_diffed;
use crate::fs_helpers::git_ls_files;
use crate::Xtask;
use clap::Parser;
use std::path::PathBuf;

const PATH_TO_HOUSE_RULES_RS: &str = file!(); // used by `cfg_target_arch`

mod autogen_comment;
mod cfg_target_arch;
mod copyright;
mod crate_name_nodash;
mod package_info;
mod repr_packed;
mod trailing_newline;
mod unsafe_code_comment;

#[derive(Parser)]
#[clap(about = r#"Collection of misc formatting "house rules"

RULES:

    - enforce the presence of the standard Microsoft copyright header
    - enforce in-repo crate names don't use '-' in their name (use '_' instead)
    - enforce Cargo.toml files don't include autogenerated "see more keys" comments
    - enforce Cargo.toml files don't contain author or version fields
    - enforce files end with a single trailing newline
    - deny usage of `#[repr(packed)]` (you want `#[repr(C, packed)]`)
    - justify usage of `cfg(target_arch = ...)` (use `guest_arch` instead!)
    - justify usage of `allow(unsafe_code)` with an UNSAFETY comment
    "#)]
pub struct HouseRules {
    /// Attempt to fix formatting issues
    #[clap(long)]
    pub fix: bool,

    /// Only run checks on files that are currently diffed
    #[clap(long, conflicts_with = "files")]
    pub only_diffed: bool,

    /// A list of files to check
    ///
    /// If no files were provided, all files in-tree will be checked
    pub files: Vec<PathBuf>,

    /// Don't run the copyright header check
    #[clap(long)]
    pub skip_copyright: bool,

    /// Don't run the autogenerated Cargo.toml "see more keys" comment check
    #[clap(long)]
    pub skip_autogen_comment: bool,

    /// Don't run the Cargo.toml author and version field checks
    #[clap(long)]
    pub skip_package_info: bool,

    /// Don't run the trailing newline check
    #[clap(long)]
    pub skip_trailing_newline: bool,

    /// Don't run the crate name check
    #[clap(long)]
    pub skip_crate_name: bool,

    /// Don't run the `#[repr(packed)]` check
    #[clap(long)]
    pub skip_repr_packed: bool,

    /// Don't run the `#[cfg(target_arch)]` check
    #[clap(long)]
    pub skip_cfg_target_arch: bool,

    /// Don't run the `#[allow(unsafe_code)]` comment check
    #[clap(long)]
    pub skip_unsafe_code_comment: bool,
}

impl HouseRules {
    /// Initialize `HouseRules` with all passes enabled
    pub fn all_passes(fix: bool, only_diffed: bool) -> HouseRules {
        HouseRules {
            fix,
            only_diffed,
            files: Vec::new(),
            skip_copyright: false,
            skip_autogen_comment: false,
            skip_package_info: false,
            skip_trailing_newline: false,
            skip_crate_name: false,
            skip_repr_packed: false,
            skip_cfg_target_arch: false,
            skip_unsafe_code_comment: false,
        }
    }
}

#[derive(Debug)]
enum Files {
    All,
    OnlyDiffed,
    Specific(Vec<PathBuf>),
}

impl Xtask for HouseRules {
    fn run(self, ctx: crate::XtaskCtx) -> anyhow::Result<()> {
        let files = if self.only_diffed {
            Files::OnlyDiffed
        } else if self.files.is_empty() {
            Files::All
        } else {
            Files::Specific(self.files)
        };

        log::trace!("running house-rules on {:?}", files);

        let files = match files {
            Files::All => git_ls_files()?,
            Files::OnlyDiffed => git_diffed(ctx.in_git_hook)?,
            Files::Specific(files) => files,
        };

        let mut errors = Vec::new();
        for path in files {
            if !self.skip_copyright {
                if let Err(e) = copyright::check_copyright(&path, self.fix) {
                    errors.push(e)
                }
            }

            if !self.skip_autogen_comment {
                if let Err(e) = autogen_comment::check_autogen_comment(&path, self.fix) {
                    errors.push(e)
                }
            }

            if !self.skip_package_info {
                if let Err(e) = package_info::check_package_info(&path, self.fix) {
                    errors.push(e)
                }
            }

            if !self.skip_trailing_newline {
                if let Err(e) = trailing_newline::check_trailing_newline(&path, self.fix) {
                    errors.push(e)
                }
            }

            if !self.skip_crate_name {
                if let Err(e) = crate_name_nodash::check_crate_name_nodash(&path) {
                    errors.push(e)
                }
            }

            if !self.skip_repr_packed {
                if let Err(e) = repr_packed::check_repr_packed(&path, self.fix) {
                    errors.push(e)
                }
            }

            if !self.skip_cfg_target_arch {
                if let Err(e) = cfg_target_arch::check_cfg_target_arch(&path, self.fix) {
                    errors.push(e)
                }
            }

            if !self.skip_unsafe_code_comment {
                if let Err(e) = unsafe_code_comment::check_unsafe_code_comment(&path, self.fix) {
                    errors.push(e)
                }
            }
        }

        for e in &errors {
            log::error!("{:#}", e);
        }

        if !errors.is_empty() && !self.fix {
            Err(anyhow::anyhow!("`house-rules` found formatting errors"))
        } else {
            Ok(())
        }
    }
}