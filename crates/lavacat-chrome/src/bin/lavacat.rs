//! `lavacat` 可执行入口：启动浏览器窗口。

fn main() {
    if let Err(error) = lavacat_chrome::run() {
        eprintln!("lavacat: {error}");
        std::process::exit(1);
    }
}
