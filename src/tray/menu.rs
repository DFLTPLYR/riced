use std::collections::HashMap;
use zbus::zvariant::OwnedValue;

pub(super) type Layout = (i32, HashMap<String, OwnedValue>, Vec<OwnedValue>);

#[derive(Debug, Clone, Default)]
pub(crate) struct MenuNode {
    pub id: i32,
    pub label: String,
    pub kind: String,
    pub enabled: bool,
    pub visible: bool,
    pub toggle_type: String,
    pub toggle_state: i32,
    pub icon_name: String,
    pub icon: Option<iced::widget::image::Handle>,
    pub children: Vec<MenuNode>,
    pub submenu: bool,
}

impl MenuNode {
    pub(super) fn parse(
        (id, props, children): Layout,
        depth: usize,
        theme_paths: &[std::path::PathBuf],
    ) -> Self {
        let text = |key: &str| {
            props
                .get(key)
                .and_then(|v| <&str>::try_from(v).ok())
                .unwrap_or("")
                .to_owned()
        };
        let flag = |key: &str| {
            props
                .get(key)
                .and_then(|v| bool::try_from(v).ok())
                .unwrap_or(true)
        };
        let icon_name = text("icon-name");
        let icon = props
            .get("icon-data")
            .and_then(|v| v.try_clone().ok())
            .and_then(|v| Vec::<u8>::try_from(v).ok())
            .and_then(|bytes| image::load_from_memory(&bytes).ok())
            .map(|image| {
                let rgba = image.to_rgba8();
                iced::widget::image::Handle::from_rgba(rgba.width(), rgba.height(), rgba.into_raw())
            })
            .or_else(|| crate::icons::image(&icon_name, 24, theme_paths));
        Self {
            id,
            label: text("label"),
            kind: text("type"),
            enabled: flag("enabled"),
            visible: flag("visible"),
            toggle_type: text("toggle-type"),
            toggle_state: props
                .get("toggle-state")
                .and_then(|v| i32::try_from(v).ok())
                .unwrap_or(-1),
            icon_name,
            icon,
            submenu: text("children-display") == "submenu" || !children.is_empty(),
            children: if depth < 64 {
                children
                    .into_iter()
                    .filter_map(|v| Layout::try_from(v).ok())
                    .map(|layout| Self::parse(layout, depth + 1, theme_paths))
                    .collect()
            } else {
                Vec::new()
            },
        }
    }

    pub(super) fn actionable(&self, id: i32) -> bool {
        self.visible
            && self.enabled
            && (self.id == id && self.kind != "separator"
                || self.children.iter().any(|child| child.actionable(id)))
    }
}

pub(super) async fn refresh(
    proxy: &zbus::Proxy<'_>,
    theme_paths: &[std::path::PathBuf],
) -> zbus::Result<MenuNode> {
    let (_, layout): (u32, Layout) = proxy
        .call("GetLayout", &(0_i32, -1_i32, Vec::<String>::new()))
        .await?;
    Ok(MenuNode::parse(layout, 0, theme_paths))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn layout_keeps_nested_toggle_state_and_protocol_defaults() {
        let child: Layout = (
            7,
            HashMap::from([
                (
                    "label".into(),
                    OwnedValue::from(zbus::zvariant::Str::from("_Enabled")),
                ),
                ("toggle-state".into(), OwnedValue::from(1_i32)),
            ]),
            vec![],
        );
        let variant = OwnedValue::try_from(zbus::zvariant::Value::from(child)).unwrap();
        let root = MenuNode::parse((0, HashMap::new(), vec![variant]), 0, &[]);
        assert_eq!(
            (
                root.children[0].id,
                root.children[0].label.as_str(),
                root.children[0].toggle_state,
                root.children[0].enabled,
                root.children[0].visible
            ),
            (7, "_Enabled", 1, true, true)
        );
    }
    #[test]
    fn disabled_parent_blocks_descendant_actions() {
        let root = MenuNode {
            enabled: false,
            visible: true,
            children: vec![MenuNode {
                id: 3,
                enabled: true,
                visible: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(!root.actionable(3));
    }
}
