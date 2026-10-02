use std::sync::LazyLock;

static LOGGER_INIT: LazyLock<()> = LazyLock::new(|| {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Debug)
            .with_tag("EasyTier-JNI"),
    );
});

pub(crate) fn init() {
    LazyLock::force(&LOGGER_INIT);
}
