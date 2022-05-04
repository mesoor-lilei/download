use std::time::Instant;

use anyhow::anyhow;
use hyper::body::HttpBody;
use hyper::client::HttpConnector;
use hyper::header::{ACCEPT_RANGES, CONTENT_LENGTH};
use hyper::{Body, Client, Method, Request, Response};
use hyper_tls::HttpsConnector;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use lazy_static::lazy_static;
use tokio::fs::{create_dir, remove_dir_all, File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::spawn;
use tokio::task::JoinHandle;

use crate::config::Config;
use crate::Result;

lazy_static! {
    static ref CONFIG: Config = Config::get().unwrap();
    static ref PROGRESS: MultiProgress = MultiProgress::new();

    /// HTTPS 客户端
    static ref CLIENT: Client<HttpsConnector<HttpConnector>> =
        Client::builder().build(HttpsConnector::new());
}

fn add_bar(size: u64, message: String, template: &str) -> Result<ProgressBar> {
    let bar = PROGRESS.add(ProgressBar::new(size));
    bar.set_style(
        ProgressStyle::default_bar()
            .template(template)?
            .progress_chars("#>-"),
    );
    bar.set_message(message);
    Ok(bar)
}

/// 下载文件进度条样式
fn add_download_bar(size: u64, task_index: usize) -> Result<ProgressBar> {
    add_bar(
        size,
        format!("任务 {} 下载中", task_index),
        "[{bar:50.cyan/blue}] [{msg}] [{bytes}/{total_bytes}] ({eta})",
    )
}

/// 合并文件进度条样式
fn add_merge_bar(size: u64) -> Result<ProgressBar> {
    add_bar(
        size,
        "合并文件中".into(),
        "[{bar:50.magenta/cyan}] [{msg}] ({eta})",
    )
}

/// 下载文件
fn download_block(
    index: (usize, usize),
    start: usize,
    block_size: usize,
    bar: ProgressBar,
) -> JoinHandle<Result> {
    spawn(async move {
        let request = Request::builder()
            .method(Method::GET)
            .header(
                "range",
                format!("bytes={}-{}", start, start + block_size - 1),
            )
            .uri(&CONFIG.uri)
            .body(Body::empty())?;
        let response = CLIENT.request(request).await?;
        write_file(response, index.0, &bar).await?;
        bar.finish_with_message(format!("任务 {} 下载完成", index.1));
        Ok(())
    })
}

/// 写入文件
async fn write_file(mut response: Response<Body>, index: usize, bar: &ProgressBar) -> Result {
    let path_buf = CONFIG.temp_file_dir.join(index.to_string());
    // 数据流方式读取响应体
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path_buf)
        .await?;
    while let Some(next) = response.data().await {
        let bytes = next?;
        bar.inc(bytes.len() as u64);
        file.write_all(&bytes).await?;
    }
    Ok(())
}

/// 合并文件
async fn merge_file(size: u64) -> Result {
    let bar = add_merge_bar(size)?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&CONFIG.file_path)
        .await?;
    for i in 0..CONFIG.size {
        let mut block_file = File::open(CONFIG.temp_file_dir.join(i.to_string())).await?;

        let size = block_file.metadata().await?.len();
        const BUF_SIZE: u64 = 1024;
        let count = size / BUF_SIZE;
        let first_buf_size = size % BUF_SIZE;

        async fn write_block(
            block_file: &mut File,
            file: &mut File,
            bar: &ProgressBar,
            buffer: &mut [u8],
        ) -> Result {
            block_file.read_exact(buffer).await?;
            bar.inc(buffer.len() as u64);
            file.write_all(buffer).await?;
            Ok(())
        }

        // 第一个块获取 `余数` 个字节
        let mut buffer = vec![0; first_buf_size as usize];
        write_block(&mut block_file, &mut file, &bar, &mut buffer).await?;

        // 剩余块获取 `BUF_SIZE` 个字节
        let mut buffer = [0; BUF_SIZE as usize];
        for _ in 0..count {
            write_block(&mut block_file, &mut file, &bar, &mut buffer).await?;
        }
    }
    bar.finish_with_message("合并文件完成");
    // 删除临时文件目录
    remove_dir_all(&CONFIG.temp_file_dir).await?;
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
    create_dir(&CONFIG.temp_file_dir).await?;

    // 单个任务下载的数据大小
    let block_size = content_length / CONFIG.size;

    // 第一个块获取 `block_size + 余数` 个字节
    let first_attach = content_length % CONFIG.size;
    let first_block_size = block_size + first_attach;
    let first_bar = add_download_bar(first_block_size as u64, 1)?;
    let mut handles = vec![download_block((0, 1), 0, first_block_size, first_bar)];

    let block_size_u64 = block_size as u64;
    // 剩余块获取 `block_size` 个字节
    for i in 1..CONFIG.size {
        let task_index = i + 1;
        let bar = add_download_bar(block_size_u64, task_index)?;
        let start = i * block_size + first_attach;
        handles.push(download_block((i, task_index), start, block_size, bar));
    }
    // 等待所有任务结束
    for handle in handles {
        handle.await??;
    }
    merge_file(content_length as u64).await?;
    println!("耗时：{:?}", start.elapsed());
    Ok(())
}
