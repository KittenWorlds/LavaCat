#![forbid(unsafe_code)]
mod peer;
fn main() {
    peer::serve(u32::MAX);
}
