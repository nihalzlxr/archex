use crate::core::db::{Db, Module};
use glob::Pattern;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;
use serde::Serialize;

pub struct Parser {
    db: Db,
}

#[derive(Debug, Serialize)]
pub struct ScanResult {
    pub files_scanned: usize,
    pub files_mapped: usize,
    pub files_unmapped: Vec<PathBuf>,
}

const SKIP_DIRS: &[&str] = &["node_modules", ".next", ".git", "dist", "build", "target"];

const VALID_EXTENSIONS: &[&str] = &["ts", "tsx", "js", "jsx"];

impl Parser {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    pub fn scan(&self, project_root: &Path) -> Result<ScanResult, Box<dyn std::error::Error>> {
        let modules = self.db.get_all_modules().map_err(|e| format!("DB error: {}", e))?;

        let mut compiled_patterns: Vec<(Module, Pattern)> = modules
            .into_iter()
            .filter_map(|m| {
                Pattern::new(&m.path_pattern)
                    .ok()
                    .map(|p| (m, p))
            })
            .collect();

        // Sort by pattern length descending (most specific first)
        compiled_patterns.sort_by(|a, b| b.1.as_str().len().cmp(&a.1.as_str().len()));

        let mut files_scanned = 0;
        let mut files_mapped = 0;
        let mut files_unmapped = Vec::new();

        let total_entries: usize = WalkDir::new(project_root)
            .into_iter()
            .filter_entry(|e| !Self::should_skip(e))
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .filter(|e| Self::has_valid_extension(e.path()))
            .count();

        eprintln!("Scanning {} files...", total_entries);

        let project_root_str = project_root.to_string_lossy().to_lowercase();

        for entry in WalkDir::new(project_root)
            .into_iter()
            .filter_entry(|e| !Self::should_skip(e))
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .filter(|e| Self::has_valid_extension(e.path()))
        {
            files_scanned += 1;

            let file_path = entry.path();
            let file_path_str = file_path.to_string_lossy();

            let relative_path = if file_path_str.to_lowercase().starts_with(&project_root_str) {
                file_path_str[project_root_str.len()..].trim_start_matches(['/', '\\'])
            } else {
                file_path_str.as_ref()
            }.to_string();

            let relative_path = relative_path.replace('\\', "/");

            if relative_path.is_empty() {
                continue;
            }

            let matched_module = compiled_patterns
                .iter()
                .find(|(_, pattern)| pattern.matches(&relative_path));

            if let Some((module, _)) = matched_module {
                self.db.upsert_file(&relative_path, module.id)?;
                files_mapped += 1;
            } else {
                files_unmapped.push(PathBuf::from(&relative_path));
            }

            if files_scanned % 50 == 0 {
                eprintln!("Processed {}/{} files", files_scanned, total_entries);
            }
        }

        eprintln!("Scan complete: {} scanned, {} mapped, {} unmapped",
            files_scanned, files_mapped, files_unmapped.len());

        Ok(ScanResult {
            files_scanned,
            files_mapped,
            files_unmapped,
        })
    }

    fn should_skip(entry: &walkdir::DirEntry) -> bool {
        if entry.file_type().is_dir() {
            let name = entry.file_name().to_string_lossy();
            SKIP_DIRS.contains(&name.as_ref())
        } else {
            false
        }
    }

    fn has_valid_extension(path: &Path) -> bool {
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| VALID_EXTENSIONS.contains(&ext))
            .unwrap_or(false)
    }
}