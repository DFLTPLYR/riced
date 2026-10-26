//! One named, app-owned modal card on an output-local overlay layer.
use super::*;
use crate::framework::AppPopup;
use iced::{Point, Size};
use iced_exwlshell::reexport::{
    Anchor, KeyboardInteractivity, Layer, LayerSize, NewLayerShellSettings, OutputOption,
};

pub(super) struct Popup {
    pub surface: window::Id,
    pub root: AppPopup,
    /// Surface id of the layer that declares this popup.
    pub owner: String,
    pub generation: u64,
    pub lists: Lists,
    pub frame: SharedFrame,
    pub revision: u64,
}

fn settings(output: OutputId) -> NewLayerShellSettings {
    NewLayerShellSettings {
        anchor: Anchor::all(),
        layer: Layer::Overlay,
        size: LayerSize::FILL,
        exclusive_zone: Some(-1),
        output_option: OutputOption::GlobalName(output.0),
        keyboard_interactivity: KeyboardInteractivity::Exclusive,
        namespace: Some("riced:app:popup".into()),
        ..Default::default()
    }
}

pub(super) fn detach(
    instance: &mut Instance,
    motion: &mut crate::ui::anim::AnimRuntime,
) -> Option<window::Id> {
    let mut popup = instance.popup.take()?;
    popup.lists.clear(motion);
    instance.hover.clear();
    Some(popup.surface)
}

pub(super) fn reconcile(
    plots: &mut Plots,
    instance: &mut Instance,
    output: OutputId,
    id: &str,
    commands: &mut Vec<Task<Plant>>,
) -> bool {
    // Any layer may declare the app's single popup; find it and remember which
    // layer owns it so the popup follows that layer's lifecycle.
    let owner = instance
        .roots
        .iter()
        .find(|(_, layer)| layer.root.popup.is_some())
        .map(|(id, _)| id.clone());
    let desired = owner.as_ref().and_then(|id| {
        instance
            .roots
            .get(id)
            .and_then(|layer| layer.root.popup.as_ref())
            .map(|popup| popup.geometry.id.clone())
    });
    if desired.is_none() {
        instance.dismissed_popup = None;
    }
    let desired =
        desired.filter(|popup| instance.dismissed_popup.as_deref() != Some(popup.as_str()));
    let generation = instance
        .runtime
        .as_ref()
        .and_then(LuaRuntime::managed_generation)
        .unwrap_or_default();
    let replace = instance.popup.as_ref().is_some_and(|popup| {
        desired
            .as_ref()
            .is_none_or(|id| *id != popup.root.geometry.id)
            || popup.generation != generation
    });
    let mut changed = false;
    if replace && let Some(surface) = detach(instance, &mut plots.animation.motion) {
        commands.push(close(plots, surface));
        changed = true;
    }
    // Dismissed or no longer declared: nothing to show or create.
    if desired.is_none() {
        return changed;
    }
    let Some(owner) = owner else {
        return changed;
    };
    let Some(root) = instance
        .roots
        .get(&owner)
        .and_then(|layer| layer.root.popup.as_ref())
    else {
        return changed;
    };
    if let Some(popup) = &mut instance.popup {
        if popup.root != *root {
            let frame = popup
                .frame
                .lock()
                .ok()
                .and_then(|f| f.last.clone())
                .unwrap_or_default();
            if let Err(error) = popup.lists.synchronize(
                Some(&popup.root.content),
                &root.content,
                &frame,
                &mut plots.animation.motion,
                Duration::from_millis(250),
            ) {
                eprintln!("app popup: {error}");
                return changed;
            }
            popup.root = root.clone();
            popup.owner = owner;
            popup.revision = popup.revision.wrapping_add(1);
            commands.push(Task::done(Plant::App(AppMessage::Redraw(popup.surface))));
        }
    } else {
        let surface = window::Id::unique();
        let mut lists = Lists::default();
        lists.scope = format!("app:{}:{id}:popup:{}", output.0, root.geometry.id);
        if let Err(error) = lists.synchronize(
            None,
            &root.content,
            &Default::default(),
            &mut plots.animation.motion,
            Duration::from_millis(250),
        ) {
            eprintln!("app popup: {error}");
            return changed;
        }
        instance.popup = Some(Popup {
            surface,
            root: root.clone(),
            owner,
            generation,
            lists,
            frame: Default::default(),
            revision: 1,
        });
        plots.windows.ids.insert(surface, PlotInfo::App(output));
        commands.push(Task::done(Plant::NewLayerShell {
            id: surface,
            settings: settings(output),
        }));
        changed = true;
    }
    if let Some(popup) = &mut instance.popup {
        for id in popup.lists.take_resets() {
            commands.push(iced::widget::operation::scroll_to(
                id,
                iced::widget::operation::AbsoluteOffset { x: 0.0, y: 0.0 },
            ));
        }
    }
    changed
}

pub(crate) fn is_popup(plots: &Plots, surface: window::Id) -> bool {
    plots
        .apps
        .instances
        .values()
        .any(|i| i.popup.as_ref().is_some_and(|p| p.surface == surface))
}

pub(crate) fn close_children(plots: &mut Plots, parent: window::Id) -> Task<Plant> {
    let mut state = std::mem::take(&mut plots.apps);
    let child = state
        .instances
        .values_mut()
        .find(|i| i.root_by_surface(parent).is_some())
        .and_then(|i| detach(i, &mut plots.animation.motion));
    plots.apps = state;
    child.map_or_else(Task::none, |child| close(plots, child))
}

pub(crate) fn dismiss(
    plots: &mut Plots,
    surface: window::Id,
    reason: &str,
    native_close: bool,
) -> Task<Plant> {
    let mut state = std::mem::take(&mut plots.apps);
    let Some(instance) = state
        .instances
        .values_mut()
        .find(|i| i.popup.as_ref().is_some_and(|p| p.surface == surface))
    else {
        plots.apps = state;
        return Task::none();
    };
    let id = instance
        .popup
        .as_ref()
        .map(|p| p.root.geometry.id.clone())
        .unwrap_or_default();
    detach(instance, &mut plots.animation.motion);
    instance.dismissed_popup = Some(id.clone());
    let parent = instance
        .roots
        .values()
        .find(|root| root.root.popup.is_some())
        .and_then(|root| root.surface);
    let result = instance.runtime.as_mut().map(|runtime| {
        runtime.app_event(
            &services(plots),
            &instance.output_name,
            &instance.context,
            &instance.props,
            AppEvent::PopupClosed { id: &id, reason },
        )
    });
    plots.apps = state;
    let close = if native_close {
        close(plots, surface)
    } else {
        plots.forget_window(surface);
        Task::none()
    };
    let command = match result {
        Some(Ok(value)) => super::super::actions::command_from_action(&value, plots),
        Some(Err(error)) => {
            eprintln!("app popup dismissal: {error}");
            Task::none()
        }
        None => Task::none(),
    };
    let refresh = parent.map_or_else(Task::none, |parent| refresh_surface(plots, parent));
    Task::batch([
        close,
        command,
        refresh,
        Task::done(Plant::App(AppMessage::Changed)),
    ])
}

fn bounds(root: &AppPopup, viewport: Size) -> (Point, Size) {
    let size = Size::new(
        (root.geometry.width as f32).min(viewport.width).max(1.0),
        (root.geometry.height as f32).min(viewport.height).max(1.0),
    );
    let point = Point::new(
        root.geometry
            .x
            .clamp(0.0, (viewport.width - size.width).max(0.0)),
        root.geometry
            .y
            .clamp(0.0, (viewport.height - size.height).max(0.0)),
    );
    (point, size)
}

pub(super) fn view(plots: &Plots, instance: &Instance, popup: &Popup) -> Element<'static, Plant> {
    use iced::widget::{Space, container, mouse_area, opaque, stack};
    let surface = popup.surface;
    let sender = Arc::new(move |action| Plant::App(AppMessage::Action { surface, action }));
    let hover = Arc::new(move |list, key, entered| {
        Plant::App(AppMessage::Hover {
            surface,
            list,
            key,
            entered,
        })
    });
    // Popups are child surfaces: they never host desktop-widget slots.
    let mut native = crate::ui::build::no_native_slots::<Plant>();
    let content = match popup.lists.build(
        &popup.root.content,
        &plots.animation.motion,
        sender,
        Some(hover),
        &mut native,
    ) {
        Ok(content) => content,
        Err(error) => {
            eprintln!("app popup view: {error}");
            return Space::new().into();
        }
    };
    let viewport = Size::new(
        instance.props["output"]["width"].as_f64().unwrap_or(1920.0) as f32,
        instance.props["output"]["height"]
            .as_f64()
            .unwrap_or(1080.0) as f32,
    );
    let (origin, size) = bounds(&popup.root, viewport);
    let revision = popup.revision;
    let content = measured::root(
        container(content)
            .width(size.width)
            .height(size.height)
            .into(),
        popup.frame.clone(),
        revision,
        Arc::new(move |data| {
            Plant::App(AppMessage::Measured {
                surface,
                revision,
                data,
            })
        }),
    );
    let dismiss = Plant::App(AppMessage::DismissPopup {
        surface,
        reason: "outside",
    });
    let backdrop = mouse_area(Space::new().width(iced::Fill).height(iced::Fill))
        .on_press(dismiss.clone())
        .on_right_press(dismiss.clone())
        .on_middle_press(dismiss);
    let card = crate::ui::widgets::anchored::anchored(
        opaque(content),
        origin,
        Point::ORIGIN,
        Default::default(),
    );
    stack![backdrop, card].into()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hosted_popup_is_modal_without_reserving_output_space() {
        let settings = settings(OutputId(7));
        assert_eq!(settings.layer, Layer::Overlay);
        assert_eq!(settings.anchor, Anchor::all());
        assert_eq!(settings.exclusive_zone, Some(-1));
        assert_eq!(
            settings.keyboard_interactivity,
            KeyboardInteractivity::Exclusive
        );
        assert_eq!(settings.output_option, OutputOption::GlobalName(7));
        assert!(!settings.events_transparent);
    }

    #[test]
    fn popup_position_and_size_are_clamped_to_the_output() {
        let lua = mlua::Lua::new();
        crate::ui::dsl::inject_ui_base(&lua).unwrap();
        let value = lua.load("return iced.layer({width=100,height=32,content=iced.text('parent'),popup={x=999,y=-10,width=400,height=900,content=iced.text('menu')}})").eval().unwrap();
        let popup = LayerRoot::parse(&lua, value, &Default::default())
            .unwrap()
            .unwrap()
            .popup
            .unwrap();
        assert_eq!(
            bounds(&popup, Size::new(1000.0, 600.0)),
            (Point::new(600.0, 0.0), Size::new(400.0, 600.0))
        );
    }
}
