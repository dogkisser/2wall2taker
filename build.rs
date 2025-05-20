fn main() {
    println!("cargo:rerun-if-changed=src/ui.fl");

    #[cfg(target_os = "windows")] {
        println!("cargo:rerun-if-changed=src/2wall2taker.rc");
        println!("cargo:rerun-if-changed=assets/eggplant.ico");
        embed_resource::compile("src/2wall2taker.rc", embed_resource::NONE)
            .manifest_optional()
            .unwrap();
    }
}