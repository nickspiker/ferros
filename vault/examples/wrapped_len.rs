//! Print the wrapped file length a VSF vault header claims (debug aid for the on-disk state).
fn main() {
    let path = std::env::args().nth(1).expect("path to a header block");
    let bytes = std::fs::read(&path).expect("read");
    match ferros_vault::vsf_wrapper::wrapped_len(&bytes) {
        Ok(n) => println!("file_length = A#{n} (G#{n:X})"),
        Err(e) => println!("header decode failed: {e:?}"),
    }
}
