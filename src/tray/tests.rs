//! Exercise real wire signatures on a private session bus, never the user's bus.
use super::*;
use std::{
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicI32, Ordering},
    },
};

struct Bus(Child);
impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Item(Arc<AtomicI32>, String);
#[interface(name = "org.kde.StatusNotifierItem")]
impl Item {
    #[zbus(property)]
    fn icon_name(&self) -> &str {
        "steam_tray_mono"
    }
    #[zbus(property)]
    fn icon_theme_path(&self) -> String {
        if self.0.load(Ordering::SeqCst) == 42 {
            format!("{}/changed", self.1)
        } else {
            self.1.clone()
        }
    }
    #[zbus(property)]
    fn attention_icon_name(&self) -> &str {
        "steam_tray_mono"
    }
    #[zbus(property)]
    fn overlay_icon_name(&self) -> &str {
        "steam_tray_mono"
    }
    #[zbus(property)]
    fn id(&self) -> &str {
        "test-item"
    }
    #[zbus(property)]
    fn title(&self) -> &str {
        "Test tray"
    }
    #[zbus(property)]
    fn status(&self) -> &str {
        if self.0.load(Ordering::SeqCst) == 42 {
            "Passive"
        } else {
            "Active"
        }
    }
    #[zbus(property)]
    fn menu(&self) -> zbus::zvariant::ObjectPath<'_> {
        "/Menu".try_into().unwrap()
    }
    #[zbus(property)]
    fn icon_pixmap(&self) -> Pixmaps {
        vec![(1, 1, vec![255, 20, 40, 60])]
    }
    fn activate(&self, x: i32, _y: i32) {
        self.0.store(x, Ordering::SeqCst);
    }
}

struct Menu(Arc<AtomicI32>);
#[interface(name = "com.canonical.dbusmenu")]
impl Menu {
    fn get_layout(
        &self,
        _parent: i32,
        _depth: i32,
        _properties: Vec<String>,
    ) -> (u32, menu::Layout) {
        let label = if self.0.load(Ordering::SeqCst) == 1 {
            "Updated"
        } else {
            "Initial"
        };
        let child: menu::Layout = (
            7,
            HashMap::from([(
                "label".into(),
                OwnedValue::from(zbus::zvariant::Str::from(label)),
            )]),
            Vec::new(),
        );
        (
            1,
            (
                0,
                HashMap::new(),
                vec![OwnedValue::try_from(zbus::zvariant::Value::from(child)).unwrap()],
            ),
        )
    }
    fn about_to_show(&self, _id: i32) -> bool {
        self.0.store(1, Ordering::SeqCst);
        true
    }
    fn event(&self, id: i32, event: &str, _data: OwnedValue, _timestamp: u32) {
        assert_eq!(event, "clicked");
        self.0.store(id, Ordering::SeqCst);
    }
}

async fn until(mut ready: impl FnMut() -> bool) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !ready() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("tray state did not converge");
}

#[test]
fn watcher_tracks_items_refreshes_dynamic_menus_delivers_actions_and_removes_exited_apps() {
    let icons = std::env::temp_dir().join(format!("riced-tray-icons-{}", std::process::id()));
    std::fs::create_dir_all(icons.join("changed")).unwrap();
    let icon = icons.join("steam_tray_mono.png");
    image::RgbaImage::new(8, 8).save(&icon).unwrap();
    image::RgbaImage::new(8, 8)
        .save(icons.join("changed/steam_tray_mono.png"))
        .unwrap();
    let mut daemon = Bus(Command::new("dbus-daemon")
        .args(["--session", "--nofork", "--print-address=1"])
        .stdout(Stdio::piped())
        .spawn()
        .expect("dbus-daemon is required for tray integration tests"));
    let mut address = String::new();
    BufReader::new(daemon.0.stdout.take().unwrap())
        .read_line(&mut address)
        .unwrap();
    let address = address.trim().to_owned();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let cache = TrayCache::default();
            let (tx, _events) = mpsc::channel(32);
            let host = {
                let address = address.clone();
                let cache = cache.clone();
                tokio::spawn(async move {
                    let mut tx = tx;
                    run(
                        &cache,
                        &mut tx,
                        zbus::connection::Builder::address(address.as_str()).unwrap(),
                    )
                    .await
                    .unwrap();
                })
            };
            until(|| cache.0.lock().unwrap().commands.is_some()).await;
            let called = Arc::new(AtomicI32::new(0));
            let menu_state = Arc::new(AtomicI32::new(0));
            let peer = zbus::connection::Builder::address(address.as_str())
                .unwrap()
                .serve_at(
                    "/Item",
                    Item(called.clone(), icons.to_string_lossy().into_owned()),
                )
                .unwrap()
                .serve_at("/Menu", Menu(menu_state.clone()))
                .unwrap()
                .build()
                .await
                .unwrap();
            let watcher = zbus::Proxy::new(&peer, WATCHER, PATH, WATCHER)
                .await
                .unwrap();
            let _: () = watcher
                .call("RegisterStatusNotifierItem", &("/Item",))
                .await
                .unwrap();
            until(|| cache.snapshot().len() == 1).await;
            let item = cache.snapshot().remove(0);
            assert_eq!(item.title, "Test tray");
            assert!(item.icon_pixmap.is_some());
            assert_eq!(item.icon_path.as_deref(), icon.to_str());
            assert!(item.attention_icon.is_some());
            assert!(item.overlay_icon.is_some());
            assert_eq!(item.menu.unwrap().children[0].label, "Initial");
            let registered: Vec<String> = watcher
                .get_property("RegisteredStatusNotifierItems")
                .await
                .unwrap();
            assert_eq!(registered, std::slice::from_ref(&item.id));
            // Re-registering cannot duplicate an item.
            let _: () = watcher
                .call("RegisterStatusNotifierItem", &("/Item",))
                .await
                .unwrap();
            cache
                .command(TrayCommand {
                    id: item.id.clone(),
                    action: TrayAction::Open(0),
                })
                .unwrap();
            until(|| cache.snapshot()[0].menu.as_ref().unwrap().children[0].label == "Updated")
                .await;
            cache
                .command(TrayCommand {
                    id: item.id.clone(),
                    action: TrayAction::MenuEvent(7),
                })
                .unwrap();
            until(|| menu_state.load(Ordering::SeqCst) == 7).await;
            cache
                .command(TrayCommand {
                    id: item.id,
                    action: TrayAction::Activate(42, 0),
                })
                .unwrap();
            until(|| called.load(Ordering::SeqCst) == 42).await;
            cache.refresh_icons();
            until(|| {
                cache.snapshot()[0].icon_path.as_deref()
                    == icons.join("changed/steam_tray_mono.png").to_str()
            })
            .await;
            peer.emit_signal(None::<()>, "/Item", ITEM, "NewStatus", &("Passive",))
                .await
                .unwrap();
            until(|| cache.snapshot()[0].status == "Passive").await;
            menu_state.store(0, Ordering::SeqCst);
            peer.emit_signal(None::<()>, "/Menu", MENU, "LayoutUpdated", &(2_u32, 0_i32))
                .await
                .unwrap();
            until(|| cache.snapshot()[0].menu.as_ref().unwrap().children[0].label == "Initial")
                .await;
            assert_eq!(cache.snapshot().len(), 1);
            // The private watcher name cannot be stolen by a second host.
            assert!(
                zbus::connection::Builder::address(address.as_str())
                    .unwrap()
                    .name(WATCHER)
                    .unwrap()
                    .build()
                    .await
                    .is_err()
            );
            drop(watcher);
            peer.close().await.unwrap();
            until(|| cache.snapshot().is_empty()).await;
            host.abort();
            let _ = host.await;
        });
    std::fs::remove_dir_all(icons).unwrap();
}
