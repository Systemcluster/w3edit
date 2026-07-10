use log::debug;

mod bundle;
mod errors;
mod hash;
mod metadata;
mod util;


fn main() {
    use bundle::*;
    use metadata::*;

    env_logger::builder()
        .is_test(false)
        .filter_level(log::LevelFilter::Trace)
        .format_timestamp(None)
        .format_module_path(false)
        .format_level(true)
        .format_target(false)
        .write_style(env_logger::WriteStyle::Auto)
        .init();

    let data = std::fs::read("test/assets/modnmm/blob0.bundle").unwrap();
    let bundle = Bundle::parse(data).unwrap();
    debug!("{:#?}", bundle);

    // for item in &bundle.items {
    //     let path = Path::new("test/assets/unpack").join(item.name.to_str().unwrap());
    //     std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    //     std::fs::write(&path, item.data().unwrap()).unwrap();
    // }

    let data = std::fs::read("test/assets/modnmm/metadata.store").unwrap();
    let metadata = Metadata::parse(&data).unwrap();
    debug!("{:#?}", metadata);
}
