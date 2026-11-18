mod app;
mod autotag;
mod library;
mod settings;
mod tags;

use app::*;
use gpui_kit::component::TitleBar;
use gpui_kit::*;

fn main() {
    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(|cx: &mut App| {
            gpui_kit::init(cx);
            gpui_kit::component::Theme::sync_system_appearance(None, cx);

            app::bind_keys(cx);
            app::enable_glass(cx);
            cx.on_action(|_: &Quit, cx| cx.quit());
            cx.set_menus(vec![
                Menu {
                    name: "easytag".into(),
                    items: vec![MenuItem::action("Quit easytag", Quit)],
                    disabled: false,
                },
                Menu {
                    name: "File".into(),
                    items: vec![
                        MenuItem::action("Open Folder…", OpenFolder),
                        MenuItem::action("Reload", Reload),
                        MenuItem::separator(),
                        MenuItem::action("Save", Save),
                    ],
                    disabled: false,
                },
                Menu {
                    name: "Edit".into(),
                    items: vec![MenuItem::action("Undo Move", UndoMove)],
                    disabled: false,
                },
                Menu {
                    name: "Tags".into(),
                    items: vec![MenuItem::action("Auto-tag", AutoTag)],
                    disabled: false,
                },
                Menu {
                    name: "View".into(),
                    items: vec![MenuItem::action("Toggle Light/Dark", ToggleTheme)],
                    disabled: false,
                },
            ]);
            cx.on_window_closed(|cx, _| cx.quit()).detach();

            // Fit the screen: never open larger than the display.
            let display = cx
                .primary_display()
                .map(|d| d.bounds().size)
                .unwrap_or(size(px(1440.), px(900.)));
            let width = px(1480.).min(display.width - px(80.));
            let height = px(920.).min(display.height - px(120.));
            let bounds = Bounds::centered(None, size(width, height), cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                // Frosted glass: the desktop shows through, blurred (Mica on Windows 11).
                window_background: if cfg!(target_os = "windows") {
                    WindowBackgroundAppearance::MicaBackdrop
                } else {
                    WindowBackgroundAppearance::Blurred
                },
                window_min_size: Some(size(px(900.), px(560.))),
                ..TitleBar::window_options()
            };
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| MainWindow::new(window, cx))
            })
            .expect("failed to open window");
            cx.activate(true);
        });
}
