//! The test grid as a program, to run the bot against it by hand:
//!
//! ```text
//! cargo run -p ferro-testgrid --example serve
//! ```
//!
//! It prints the login address and the test account, then runs until Ctrl-C. The account is the
//! test grid's own, not a Second Life one.

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let grid = match ferro_testgrid::TestGrid::start(ferro_testgrid::GridConfig::default()).await {
        Ok(grid) => grid,
        Err(e) => {
            eprintln!("the test grid could not start: {e}");
            std::process::exit(1);
        }
    };
    println!("FERRO_LOGIN_URI={}", grid.login_uri());
    println!("FERRO_USERNAME='{}'", grid.username());
    println!("FERRO_PASSWORD='{}'", grid.password());
    let _ = tokio::signal::ctrl_c().await;
}
