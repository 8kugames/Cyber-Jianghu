use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    main_fn::run().await
}

#[path = "main_support/admin_static.rs"]
mod admin_static;

#[path = "main_support/main_fn.rs"]
mod main_fn;

#[path = "main_support/router.rs"]
mod router;

#[cfg(test)]
#[path = "main_support/main_tests.rs"]
mod tests;
