use std::env;
use std::env::temp_dir;
use std::fs::create_dir;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::anyhow;
use clap::{crate_authors, crate_description, crate_name, crate_version, Arg, Command};
use hyper::client::HttpConnector;
use hyper::header::{ACCEPT_RANGES, CONTENT_LENGTH};
use hyper::{Body, Client, Method, Request, Response, Uri};
use hyper_tls::HttpsConnector;
use lazy_static::lazy_static;
use tokio::{
    fs::{self, OpenOptions},
    io::AsyncWriteExt,
    spawn,
};
use uuid::Uuid;

use crate::Result;

lazy_static! {
    static ref CONFIG: Config = Config::get().unwrap();

    /// HTTPS 客户端
    static ref CLIENT: Client<HttpsConnector<HttpConnector>> =
        Client::builder().build(HttpsConnector::new());
}

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

/// 下载文件
async fn download_block(index: usize, start: usize, block_size: usize) -> Result {
    let request = Request::builder()
        .method(Method::GET)
        .header(
            "range",
            format!("bytes={}-{}", start, start + block_size - 1),
        )
        .uri(&CONFIG.uri)
        .body(Body::empty())?;
    let response = CLIENT.request(request).await?;
    write_file(response, CONFIG.temp_file_dir.join(index.to_string())).await?;
    Ok(())
}

/// 写入文件
async fn write_file(response: Response<Body>, path_buf: PathBuf) -> Result {
    let body = hyper::body::to_bytes(response).await?;
    fs::write(&path_buf, body.iter()).await?;
    println!("Write: {}", path_buf.display());
    Ok(())
}

/// 合并文件
async fn merge_file() -> Result {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&CONFIG.file_path)
        .await?;
    for i in 0..CONFIG.size {
        let path = CONFIG.temp_file_dir.join(i.to_string());
        file.write_all(&fs::read(&path).await?).await?;
    }
    // 删除临时文件目录
    fs::remove_dir_all(&CONFIG.temp_file_dir).await?;
    println!("Remove temp file dir: {}", CONFIG.temp_file_dir.display());
    println!("合并文件完成");
    Ok(())
}

pub async fn run() -> Result {
    let start = Instant::now();
    let request = Request::builder()
        .method(Method::HEAD)
        .uri(&CONFIG.uri)
        .body(Body::empty())?;
    let response = CLIENT.request(request).await?;
    let headers = response.headers();
    let content_length = match headers.get(CONTENT_LENGTH) {
        None => return Err(anyhow!("{CONTENT_LENGTH} 为空")),
        Some(t) => t.to_str()?.parse::<usize>()?,
    };
    match headers.get(ACCEPT_RANGES) {
        None => return Err(anyhow!("不支持 {ACCEPT_RANGES} 请求")),
        Some(t) => {
            if t.to_str()? != "bytes" {
                return Err(anyhow!("不支持 {ACCEPT_RANGES} 请求"));
            }
        }
    };

    create_dir(&CONFIG.temp_file_dir)?;
    println!("Create temp file dir: {}", CONFIG.temp_file_dir.display());

    // 单个任务下载的数据大小
    let block_size = content_length / CONFIG.size;
    let first_attach = content_length % CONFIG.size;
    println!("数据块长度：{}", content_length);
    println!("单次下载数据块长度：{}", block_size);
    println!("任务 1 启动");

    // 第一个块获取 `block_size + 余数` 个字节
    let mut handles = vec![spawn(download_block(0, 0, first_attach + block_size))];

    // 剩余块获取 `block_size` 个字节
    for i in 1..CONFIG.size {
        let start = i * block_size + first_attach;
        println!("任务 {} 启动", i + 1);
        let handle = spawn(download_block(i, start, block_size));
        handles.push(handle);
    }
    // 等待所有任务结束
    for handle in handles {
        handle.await??;
    }
    merge_file().await?;
    println!("耗时：{:?}", start.elapsed());
    Ok(())
}
