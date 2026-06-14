//! Desktop integration: give every Idiolect *window* a stable app identity so the
//! GNOME dock / taskbar shows the microphone icon, not its generic fallback.
//!
//! On X11/GNOME a bare egui window is a "window-backed app": the shell ignores the
//! `_NET_WM_ICON` the toolkit sets (we do set it) and falls back to
//! `application-x-executable` — the grey cog in Ubuntu's Yaru theme. The lever the
//! shell *does* honour is a `.desktop` file whose `StartupWMClass` matches the
//! window's `WM_CLASS`: it then binds the window to that entry and paints its
//! `Icon=`. So at daemon startup we lay down one `NoDisplay` `.desktop` per windowed
//! component (basename = `WM_CLASS`, so the basename heuristic matches too) plus one
//! themed `idiolect` icon, and refresh GNOME's caches. Idempotent: only writes when
//! the on-disk content differs, so repeated daemon starts don't churn mtimes.
//!
//! Test note: the pure builders (`desktop_entry`, `icon_svg`) and the filesystem
//! `install` are unit-tested against a temp data-home below. The remaining steps —
//! refreshing the caches (external tools) and GNOME Shell re-reading the entry to
//! repaint the dock — are a live desktop boundary with no headless seam, so they are
//! exercised only by the manual check in the PR, not in CI.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use idiolect_common::config::XdgBaseDirs;

/// One windowed Idiolect component that shows up in the dock, identified by its
/// binary file name — which is **also** the window's X11 `WM_CLASS`. winit derives
/// `WM_CLASS` from `current_exe()`'s basename, NOT from the `eframe::run_native`
/// app-id, so e.g. the review dialog's class is `idiolect-review-dialog` (the
/// binary), never `idiolect-review` (its app-id). GNOME matches the window to this
/// entry by `WM_CLASS`, so the binary basename has to drive everything: the
/// `.desktop` basename, its `StartupWMClass`, and the `Exec` target. Keep them one
/// field so they can't drift — keying off the app-id instead is exactly the bug that
/// left the review/retention windows showing a generic cog.
struct Window {
    /// Binary file name beside the daemon === the window's X11 `WM_CLASS`.
    binary: &'static str,
    /// Human-facing entry name (alt-tab / window lists only; entries are `NoDisplay`,
    /// so they never appear in the apps grid).
    name: &'static str,
}

/// The windowed components, by `WM_CLASS` (= binary name). The recording indicator is
/// intentionally absent: it is a `_NET_WM_WINDOW_TYPE_NOTIFICATION` overlay with no
/// dock presence.
const WINDOWS: &[Window] = &[
    Window {
        binary: "idiolect-settings",
        name: "Idiolect — Settings",
    },
    Window {
        binary: "idiolect-review-dialog",
        name: "Idiolect — Review",
    },
    Window {
        binary: "idiolect-retention-dialog",
        name: "Idiolect — Training data",
    },
];

/// The themed icon every entry points at (`Icon=idiolect`), installed once as
/// `hicolor/scalable/apps/idiolect.svg`.
const ICON_NAME: &str = "idiolect";
/// The Idiolect accent, matching the tray glyph and the settings window icon.
const ACCENT: &str = "#7c83fd";

/// The `.desktop` body that binds one window's `WM_CLASS` to the `idiolect` icon.
/// `StartupWMClass` is the binary basename (the real `WM_CLASS`); `Exec` is required
/// by the spec and points at the real binary, though these windows are launched by
/// the daemon, never from the apps grid (`NoDisplay`).
fn desktop_entry(window: &Window, exec: &Path) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name={name}\n\
         Comment=Idiolect voice dictation\n\
         Icon={ICON_NAME}\n\
         Exec={exec}\n\
         StartupWMClass={wm_class}\n\
         NoDisplay=true\n\
         Terminal=false\n",
        name = window.name,
        exec = exec.display(),
        wm_class = window.binary,
    )
}

/// The Idiolect line-art microphone as a scalable app icon: the same geometry as the
/// settings window icon (a 64-unit grid), accent fill plus stroke.
fn icon_svg() -> String {
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"64\" height=\"64\" viewBox=\"0 0 64 64\">\n\
         <g fill=\"none\" stroke=\"{ACCENT}\" stroke-width=\"4\" stroke-linecap=\"round\" stroke-linejoin=\"round\">\n\
         <rect x=\"22\" y=\"8\" width=\"20\" height=\"30\" rx=\"10\" fill=\"{ACCENT}\"/>\n\
         <path d=\"M14 34 C14 50 50 50 50 34\"/>\n\
         <path d=\"M32 47 L32 54\"/>\n\
         <path d=\"M23 55 L41 55\"/>\n\
         </g>\n\
         </svg>\n"
    )
}

/// Write the `.desktop` entries and themed icon under `data_home`, idempotently.
/// Returns the files that were created or changed (empty when already current).
fn install(data_home: &Path, bin_dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut written = Vec::new();

    let apps = data_home.join("applications");
    for window in WINDOWS {
        let path = apps.join(format!("{}.desktop", window.binary));
        let body = desktop_entry(window, &bin_dir.join(window.binary));
        if write_if_changed(&path, body.as_bytes())? {
            written.push(path);
        }
    }

    let icon = data_home
        .join("icons/hicolor/scalable/apps")
        .join(format!("{ICON_NAME}.svg"));
    if write_if_changed(&icon, icon_svg().as_bytes())? {
        written.push(icon);
    }

    Ok(written)
}

/// Write `contents` to `path` (creating parents) only when it differs from what is
/// already there, so repeated installs don't churn mtimes. Returns whether the file
/// was created or changed.
fn write_if_changed(path: &Path, contents: &[u8]) -> io::Result<bool> {
    if fs::read(path).is_ok_and(|existing| existing == contents) {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)?;
    Ok(true)
}

/// Whether a daemon launch should install desktop integration. Only a *real,
/// persistent* launch does: config validation (`check_config`), the ephemeral test
/// daemons (`shutdown_after_client` — used solely by tests, never by the systemd
/// unit), and headless/CI runs (`tray_disabled`) must never write into the user's
/// real `~/.local/share`. This is the guard that stops `cargo test` — which spawns
/// real daemons inheriting the developer's home — from clobbering their entries.
pub(crate) fn should_install(
    check_config: bool,
    shutdown_after_client: bool,
    tray_disabled: bool,
) -> bool {
    !check_config && !shutdown_after_client && !tray_disabled
}

/// Best-effort: install the integration files and, when needed, nudge GNOME to
/// re-read them. Never fails the daemon — the dock icon is cosmetic.
pub(crate) fn ensure(xdg: &XdgBaseDirs) {
    let Some(bin_dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    else {
        return;
    };
    let written = match install(&xdg.data_home, &bin_dir) {
        Ok(written) => written,
        Err(error) => {
            eprintln!("desktop integration: install failed: {error}");
            return;
        }
    };
    // Refresh when we just wrote files, or when a *stale* icon cache is hiding our
    // icon — the case that bit us: the entry/icon are already on disk from an earlier
    // start, but a cache another app left predates the icon, so GTK never sees it.
    let hicolor = xdg.data_home.join("icons/hicolor");
    let icon = hicolor
        .join("scalable/apps")
        .join(format!("{ICON_NAME}.svg"));
    if !written.is_empty() || icon_cache_is_stale(&hicolor, &icon) {
        refresh_caches(&xdg.data_home, &hicolor);
    }
}

/// True when an `icon-theme.cache` exists but predates `icon`, so GTK serves a stale
/// listing that omits it. *No* cache at all is not stale: GTK then scans the
/// directories directly and finds the icon without help.
fn icon_cache_is_stale(hicolor: &Path, icon: &Path) -> bool {
    let mtime = |path: &Path| fs::metadata(path).and_then(|meta| meta.modified()).ok();
    icon_newer_than_cache(mtime(icon), mtime(&hicolor.join("icon-theme.cache")))
}

/// `Some(icon) > Some(cache)`; a missing icon or cache is treated as "not newer", so
/// we don't churn the cache when there is nothing to refresh.
fn icon_newer_than_cache(icon: Option<SystemTime>, cache: Option<SystemTime>) -> bool {
    matches!((icon, cache), (Some(icon), Some(cache)) if icon > cache)
}

/// Nudge GNOME to pick up the entry/icon without a re-login. `update-desktop-database`
/// may be absent (fine — the dir monitor still fires); the icon cache rebuild is the
/// one that matters, see [`icon_cache_command`].
fn refresh_caches(data_home: &Path, hicolor: &Path) {
    let _ = Command::new("update-desktop-database")
        .arg(data_home.join("applications"))
        .status();
    let _ = icon_cache_command(hicolor).status();
}

/// The argv that rebuilds the hicolor icon cache. `--ignore-theme-index` is the crux:
/// a user's `~/.local/share/icons/hicolor` commonly has a cache but no `index.theme`
/// (some other app built it), and without this flag the tool aborts with "No theme
/// index file" — leaving the stale cache to keep hiding our freshly-added icon.
fn icon_cache_command(hicolor: &Path) -> Command {
    let mut cmd = Command::new("gtk-update-icon-cache");
    cmd.arg("--force").arg("--ignore-theme-index").arg(hicolor);
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> &'static Window {
        &WINDOWS[0]
    }

    #[test]
    fn desktop_entry_binds_the_window_class_to_the_idiolect_icon() {
        let exec = Path::new("/opt/idiolect/bin/idiolect-settings");
        let entry = desktop_entry(settings(), exec);
        assert!(entry.starts_with("[Desktop Entry]"), "{entry}");
        assert!(entry.contains("Type=Application"), "{entry}");
        assert!(entry.contains("Name=Idiolect — Settings"), "{entry}");
        // The crux: the shell binds this entry to the window by WM_CLASS, then paints
        // its Icon — so both must be present and correct.
        assert!(
            entry.contains("StartupWMClass=idiolect-settings"),
            "{entry}"
        );
        assert!(entry.contains(&format!("Icon={ICON_NAME}")), "{entry}");
        // NoDisplay keeps these matcher stubs out of the apps grid.
        assert!(entry.contains("NoDisplay=true"), "{entry}");
        assert!(
            entry.contains("Exec=/opt/idiolect/bin/idiolect-settings"),
            "{entry}"
        );
    }

    #[test]
    fn entries_are_keyed_to_the_real_window_class_not_the_eframe_app_id() {
        // winit sets the X11 WM_CLASS from the binary basename, so each entry must be
        // keyed to the binary name (e.g. `idiolect-review-dialog`), NOT the
        // `run_native` app-id (`idiolect-review`). Keying off the app-id is exactly
        // what left the review/retention windows matching nothing and showing a cog.
        let classes: Vec<&str> = WINDOWS.iter().map(|w| w.binary).collect();
        assert!(classes.contains(&"idiolect-review-dialog"), "{classes:?}");
        assert!(classes.contains(&"idiolect-retention-dialog"), "{classes:?}");
        assert!(
            !classes.contains(&"idiolect-review"),
            "must be the binary name, not the app-id: {classes:?}"
        );
        // The emitted StartupWMClass and the .desktop basename both equal that class.
        for window in WINDOWS {
            let entry = desktop_entry(window, Path::new("/x").join(window.binary).as_path());
            assert!(
                entry.contains(&format!("StartupWMClass={}", window.binary)),
                "{entry}"
            );
        }
    }

    #[test]
    fn icon_svg_is_a_microphone_in_the_accent_colour() {
        let svg = icon_svg();
        assert!(svg.trim_start().starts_with("<svg"), "{svg}");
        assert!(svg.contains("viewBox"), "must scale: {svg}");
        assert!(svg.contains(ACCENT), "must be the accent colour: {svg}");
        assert!(svg.contains("</svg>"), "{svg}");
    }

    #[test]
    fn install_writes_a_desktop_entry_per_window_and_the_icon() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let data_home = tmp.path();
        let bin_dir = Path::new("/usr/lib/idiolect");

        let written = install(data_home, bin_dir).expect("install");

        let icon = data_home.join("icons/hicolor/scalable/apps/idiolect.svg");
        assert!(icon.is_file(), "icon must be installed");
        assert!(written.contains(&icon), "icon reported as written: {written:?}");

        for window in WINDOWS {
            let path = data_home
                .join("applications")
                .join(format!("{}.desktop", window.binary));
            assert!(path.is_file(), "missing entry for {}", window.binary);
            let body = fs::read_to_string(&path).expect("read entry");
            assert!(
                body.contains(&format!("StartupWMClass={}", window.binary)),
                "{body}"
            );
            assert!(body.contains("Icon=idiolect"), "{body}");
            // Exec points at the real binary beside the daemon.
            assert!(
                body.contains(&format!("Exec={}", bin_dir.join(window.binary).display())),
                "{body}"
            );
        }
        // settings + review + retention entries, plus the icon.
        assert_eq!(written.len(), WINDOWS.len() + 1, "{written:?}");
    }

    #[test]
    fn only_a_real_persistent_launch_installs_desktop_integration() {
        // The real daemon: a plain `run` (the systemd unit) installs.
        assert!(should_install(false, false, false));
        // Config validation must not touch the user's home.
        assert!(!should_install(true, false, false));
        // Ephemeral test daemons (--shutdown-after-client) must not — this is what
        // stopped `cargo test`'s spawned daemons from clobbering the real entries.
        assert!(!should_install(false, true, false));
        // Headless / CI (tray disabled) must not.
        assert!(!should_install(false, false, true));
    }

    #[test]
    fn icon_cache_rebuild_ignores_the_missing_theme_index() {
        let cmd = icon_cache_command(Path::new("/x/icons/hicolor"));
        assert_eq!(cmd.get_program().to_string_lossy(), "gtk-update-icon-cache");
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        // The crux: without this the tool refuses on a hicolor dir that has a cache
        // but no index.theme, so the stale cache keeps our icon hidden.
        assert!(args.iter().any(|a| a == "--ignore-theme-index"), "{args:?}");
        assert!(args.iter().any(|a| a == "--force"), "{args:?}");
        assert!(
            args.last().is_some_and(|a| a.ends_with("hicolor")),
            "{args:?}"
        );
    }

    #[test]
    fn stale_cache_is_only_the_icon_outliving_an_existing_cache() {
        use std::time::Duration;
        let older = SystemTime::UNIX_EPOCH;
        let newer = older + Duration::from_secs(10);
        // Icon written after the cache → stale, must rebuild.
        assert!(icon_newer_than_cache(Some(newer), Some(older)));
        // Cache at least as new as the icon → it already lists the icon.
        assert!(!icon_newer_than_cache(Some(older), Some(newer)));
        assert!(!icon_newer_than_cache(Some(older), Some(older)));
        // No cache → GTK scans the dirs directly; nothing to refresh.
        assert!(!icon_newer_than_cache(Some(newer), None));
    }

    #[test]
    fn install_is_idempotent_and_reports_only_changed_files() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let data_home = tmp.path();
        let bin_dir = Path::new("/usr/lib/idiolect");

        let first = install(data_home, bin_dir).expect("first install");
        assert!(!first.is_empty(), "first run writes everything");

        // Nothing changed on disk, so a second run writes nothing.
        let second = install(data_home, bin_dir).expect("second install");
        assert!(second.is_empty(), "idempotent: {second:?}");

        // Files still there and intact.
        assert!(data_home
            .join("icons/hicolor/scalable/apps/idiolect.svg")
            .is_file());
    }
}
