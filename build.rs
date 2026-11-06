fn main() {
    println!("cargo:rerun-if-changed=melangeur.rc");
    println!("cargo:rerun-if-changed=assets/melangeur.ico");
    embed_resource::compile("melangeur.rc", embed_resource::NONE).manifest_optional().unwrap();
}
