mod http;

use http::run;

pub(crate) type Result<T = ()> = anyhow::Result<T>;

#[tokio::main]
async fn main() -> Result {
    run().await?;
    Ok(())
}
