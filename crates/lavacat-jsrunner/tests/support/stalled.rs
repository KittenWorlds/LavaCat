#![forbid(unsafe_code)]
mod peer;
fn main() {
    peer::serve(lavacat_jsrunner::ABI_VERSION);
}
