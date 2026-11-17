fn main() {
    println!("cargo:rerun-if-changed=page/hxn_audio.mjs");
    println!("cargo:rerun-if-changed=c/agent.c");
    println!("cargo:rerun-if-changed=c/direct.c");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("emscripten") {
        return;
    }
    if std::env::var("CARGO_CFG_TARGET_FEATURE")
        .unwrap_or_default()
        .split(',')
        .any(|f| f == "atomics")
    {
        cc::Build::new()
            .file("c/agent.c")
            .flag("-pthread")
            .compile("hxn_audio_agent");
    } else {
        let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
        let agent = include_str!("page/hxn_audio.mjs")
            .replace("export ", "")
            .replace("''", "\"\"");
        let direct = include_str!("c/direct.c").replace("/*AGENT*/", &agent);
        let path = out.join("direct.c");
        std::fs::write(&path, direct).unwrap();
        cc::Build::new().file(path).compile("hxn_audio_direct");
    }
}
