use iced::{
    Size,
    window::{Settings, settings::PlatformSpecific},
};

use crate::app::App;

mod app;

fn main() -> color_eyre::Result<()> {
    let (non_blocking, _guard) = tracing_appender::non_blocking(std::io::stdout());
    tracing_subscriber::fmt()
        .with_writer(non_blocking)
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::WARN.into()),
        )
        .init();

    iced::application(App::new, App::update, App::view)
        .font(iced_fonts::LUCIDE_FONT_BYTES)
        .window(Settings {
            size: Size::new(1100.0, 700.0),
            min_size: Some(Size::new(800.0, 500.0)),
            platform_specific: platform_settings(),
            ..Default::default()
        })
        .run()?;

    Ok(())
}

#[cfg(target_os = "linux")]
fn platform_settings() -> PlatformSpecific {
    PlatformSpecific {
        application_id: "lumina".to_owned(),
        ..Default::default()
    }
}

#[cfg(target_os = "macos")]
fn platform_settings() -> PlatformSpecific {
    PlatformSpecific {
        title_hidden: false,
        titlebar_transparent: false,
        fullsize_content_view: false,
    }
}

#[cfg(target_os = "windows")]
fn platform_settings() -> PlatformSpecific {
    PlatformSpecific {
        drag_and_drop: true,
        skip_taskbar: false,
        undecorated_shadow: false,
        corner_preference: Default::default(),
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn platform_settings() -> PlatformSpecific {
    PlatformSpecific::default()
}
