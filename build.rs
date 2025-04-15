extern crate embed_resource;
fn main() {
    let _ = embed_resource::compile("resource.rc", embed_resource::NONE);
}