#![forbid(unsafe_code)]
fn main() {
    // Windows' default main-thread stack is too small for the supported 512 DOM
    // levels. Construct all !Send DOM/engine state inside this bounded worker.
    let result = std::thread::Builder::new()
        .name("jsrunner-main".into())
        .stack_size(32 * 1024 * 1024)
        .spawn(lavacat_jsrunner::run_quickjs_service)
        .expect("start runner thread")
        .join();
    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
        Err(_) => std::process::exit(1),
    }
}
