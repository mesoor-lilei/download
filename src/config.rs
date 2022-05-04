use std::env::temp_dir;
use std::path::{Path, PathBuf};

use anyhow::anyhow;
use clap::{crate_authors, crate_description, crate_name, crate_version, Arg, Command};
use hyper::Uri;
use uuid::Uuid;

use crate::Result;

pub struct Config {
    pub size: usize,
    pub uri: Uri,
    pub file_path: String,
    pub temp_file_dir: PathBuf,
}

impl Config {
    pub fn get() -> Result<Self> {
        let matches = Command::new(crate_name!())
            .version(crate_version!())
            .author(crate_authors!())
            .about(crate_description!())
            .args(&[
                Arg::new("size").help("并发任务数量").required(true),
                Arg::new("uri").help("资源 URI").required(true),
                Arg::new("file-path").help("保存文件路径").required(true),
            ])
            .get_matches();

        let size = matches.value_of_t("size")?;
        let uri = matches.value_of_t("uri")?;
        let file_path = matches.value_of_t("file-path")?;

        // 检查文件是否已存在
        if Path::new(&file_path).exists() {
            return Err(anyhow!("文件 `{}` 已存在", file_path));
        }

        let temp_file_dir = temp_dir().join(Uuid::new_v4().to_string());

        Ok(Self {
            size,
            uri,
            file_path,
            temp_file_dir,
        })
    }
}
