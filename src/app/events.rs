use iced::Event;
use iced::mouse::Button;
use iced::window::Id;
use iced_exwlshell::to_layer_message;
use iced_wayland_subscriber::OutputInfo;
use iced_wayland_subscriber::shell::ShellInfo;

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
// Payloads are forwarded from the shell subscriber; several variants are
// currently acknowledged without inspecting their contents.
#[allow(dead_code)]
pub enum LandEvent {
    NewShell(ShellInfo),
    Closed(Id),
    WindowOutputChanged {
        window: Id,
        output: Option<OutputInfo>,
    },
    OutputAdded(OutputInfo),
    OutputUpdated(OutputInfo),
    OutputRemoved(OutputInfo),
    Locked,
    LockDenied,
    LockedFinished,
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
#[allow(dead_code)]
pub enum ConfigEvent {
    ConfigTick,
    ConfigReloaded(crate::config::Config),
    /// Runtime edit from a Settings-panel control. Applied to the single
    /// live config, persisted, and broadcast via full redraw.
    Patch(crate::config::ConfigPatch),
    /// Coalesced disk write for staged `Patch` edits (carries the arm
    /// generation; stale timers are ignored). Slider drags fire dozens of
    /// patches a second — memory updates every tick, the file once idle.
    SaveTimer(u64),
    /// Slider released: persist staged edits to the config file right now
    /// instead of waiting out the coalescing timer. No-op when clean.
    SaveNow,
    /// Template re-render after a theme switch finished (sys `change_theme`
    /// equivalent). Carries per-template errors, empty when all applied.
    TemplatesDone(Vec<String>),
    /// Pending-regen countdown elapsed (carries the arm generation;
    /// stale timers are ignored).
    RegenTimer(u64),
    /// Dynamic-theme regeneration finished: fresh `dynamic.json` on disk
    /// (empty errors) or the failure reasons. Always repaints so new
    /// colors apply on the next frame.
    ThemeRegenerated(Vec<String>),
    /// `widgets.toml` changed on disk: fresh declarative widget defs.
    /// Stored live and repainted (bars re-resolve slot names).
    WidgetsReloaded(Vec<crate::config::WidgetDef>),
}

/// Top-level bar/shell event routing enum. Kept small: raw input and
/// lifecycle stay flat, everything domain-specific lives in the nested
/// [`WidgetEvent`] / [`BarEvent`] / [`StyleEvent`] enums.
#[derive(Debug, Clone)]
pub enum TopEvent {
    // Lifecycle
    /// Spawn a bar on the closest edge to the context-menu cursor.
    Sow,
    /// Remove the bar: close its window, drop tracking, and delete its
    /// `[[bar]]` entry so it stays gone after restart.
    Remove(Id),

    // Raw input
    Pressed(Id, Button),
    Released(Id, Button),

    // Domain events
    Widget(WidgetEvent),
    Bar(BarEvent),
    Style(StyleEvent),
}

/// Widget-domain events: per-widget input, the Lua timer/animation
/// frames, popup/action dispatch, and slot-widget edits.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
#[allow(dead_code)]
pub enum WidgetEvent {
    /// Left press on one widget (renderer hit-tested per-widget mouse
    /// area): records the press target so the matching release — and
    /// only it — dispatches the click. Bubbled outer releases ignore
    /// widget targets (their own handler owns the click).
    Pressed(Id, usize, String),
    /// Left release on one widget: clicks only when the press target
    /// matches (same bar/slot/widget, under the hold threshold).
    Released(Id, usize, String),
    /// 250ms Lua-widget timer: re-runs every `render()` whose interval
    /// elapsed. Never repaints by itself — emits `Changed` when an
    /// output moved.
    Tick,
    /// 16ms animation frame: advances the aura-anim runtime for list
    /// enter/exit transitions and sweeps settled ghosts. Only
    /// subscribed while a motion is active (repaint via Scope::All).
    Anim,
    /// A Lua widget output moved: repaint so bars pick the new text up.
    Changed,
    /// Click a popup item: run the widget's `on_action()` with the item
    /// key, then re-render menu and cell.
    PopupSelect(Id, String),
    /// Click a cell button (`ui.button` in a `render()` tree): run the
    /// owning widget's `on_action()` with the button key, then
    /// re-render that widget (a toggle flips its next output).
    CellAction(String, String),
    /// Widget of one slot by `widgets.toml` name (position, not
    /// label): checked appends the name, unchecked removes it. Applied
    /// live, persisted. Accepted here as a widget-domain alias of
    /// [`BarEvent::SlotWidget`]; both route to the same handler.
    SetSlotWidget(Id, usize, String, bool),
}

/// Bar/layout configuration events: geometry and per-slot layout,
/// applied live and persisted.
#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
#[allow(dead_code)]
pub enum BarEvent {
    /// Bar length as % of the output long axis (1–100): applied live.
    Length(Id, f32),
    /// Bar thickness in px (1–thin output axis): applied live.
    Thickness(Id, f32),
    /// Grid cells along the long axis (columns when horizontal, rows when
    /// vertical): applied live, persisted.
    Slots(Id, u32),
    /// Child alignment of one slot (position, not label): applied live,
    /// persisted to the bar's `[[bar]] aligns` entry.
    SlotAlign(Id, usize, crate::app::layers::top::SlotAlign),
    /// Widget of one slot by `widgets.toml` name (position, not
    /// label): checked appends the name, unchecked removes it. Applied
    /// live, persisted to the bar's `[[bar]] widgets` entry.
    SlotWidget(Id, usize, String, bool),
    /// Inset inside every slot cell (px): applied live, persisted to
    /// the bar's `[[bar]] slot_padding` entry.
    SlotPadding(Id, f32),
    /// Gap between slot cells and icon/text segments (px): applied
    /// live, persisted to the bar's `[[bar]] slot_spacing` entry.
    SlotSpacing(Id, f32),
}

/// Visual/appearance configuration events, applied live where possible
/// and persisted.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum StyleEvent {
    /// Backdrop opacity preset (0.0/0.25/0.5/0.75/1.0): single commit.
    Opacity(Id, f32),
    /// Floating look: inset the backdrop with margins (view-live padding,
    /// exclusive zone kept): applied live.
    Floating(Id, bool),
    /// Content inset in px on one edge, floating look only (view-live
    /// padding).
    Margin(Id, Edge, i32),
    /// Per-corner rounding in px (view-live).
    Radius(Id, Corner, f32),
}

/// Bar content edge for [`StyleEvent::Margin`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Top,
    Right,
    Bottom,
    Left,
}

/// Bar corner for [`StyleEvent::Radius`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
#[allow(dead_code)]
pub enum BackgroundEvent {
    SelectionTick,
    Repaint,
    Pressed(Id, Button),
    Released(Id, Button),
    /// Open a native file dialog for a new wallpaper. The async result
    /// comes back as `WallpaperPicked`: `Some(path)` on accept, `None`
    /// when the dialog is cancelled — which is a no-op by design.
    PickWallpaper,
    WallpaperPicked(Option<String>),
}

/// Notification lifecycle for the Lua-configured notification layer
/// (D-Bus server + internal events). Payloads are plain data: the
/// layer resolves output, windows, and expiry from them.
#[derive(Debug, Clone)]
pub enum NotifyEvent {
    /// A notification arrived (D-Bus, internal event, or test source).
    Arrived(crate::app::layers::notification::Notification),
    /// Dismiss one notification now (click, timeout sweep, D-Bus close,
    /// or output removal). Unknown ids are ignored.
    Dismissed(u32),
    /// Peer asked to close (D-Bus `CloseNotification`): drop silently,
    /// the `NotificationClosed` signal already went out on the bus.
    PeerClosed(u32),
    /// An action button fired: emit `ActionInvoked` for valid keys,
    /// then dismiss like a click. Unknown ids/keys are ignored.
    Invoke(u32, String),
    /// 250ms expiry sweep tick, gated on a non-empty queue like
    /// `WidgetEvent::Tick` (never repaints by itself).
    Tick,
    /// D-Bus server is up (carries nothing — the subscription owns the
    /// connection; used for logging once).
    DBusUp,
}

#[derive(Debug, Clone, Copy)]
pub enum SettingEvent {
    Select(Id, crate::app::layers::SettingPage),
    /// Pick the bar edited by the Panel page (`id` = settings window).
    SelectBar(Id, Id),
    /// Pick the wallpaper image edited below the map (`id` = settings
    /// window, `usize` = index into `[[background.image]]`).
    SelectImage(Id, usize),
    /// Pick the slot the Panel page aligns (`id` = settings window,
    /// `usize` = position into the edited bar's slots).
    SelectSlot(Id, usize),
    MapViewChanged {
        id: Id,
        view: crate::components::display_map::MapView,
    },
}

#[to_layer_message(multi)]
#[derive(Debug, Clone)]
pub enum Plant {
    // Create,Update, Delete
    Tend,
    Sprout,
    Uproot(Id),
    Graft(Id, Event),
    // File IPC poll tick (see `cli`): drains `riced.cmd` queued by CLI clients.
    IpcPoll,
    // Wayland
    Wayland(LandEvent),
    // Layers/Plots
    TopPlot(TopEvent),
    BackgroundPlot(BackgroundEvent),
    SettingPlot(SettingEvent),
    // Notifications (D-Bus server + internal events)
    Notify(NotifyEvent),
    // Config
    Config(ConfigEvent),
}
