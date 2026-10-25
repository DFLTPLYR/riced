use super::*;
use crate::framework::tasks::Owner;

struct Fixture {
    lua: Lua,
    dir: tempfile::TempDir,
    owner: Owner,
}
impl Fixture {
    fn due(&self) {
        let mut store = self.lua.app_data_mut::<Store>().unwrap();
        for document in store.documents.values_mut() {
            if let Some(watch) = document.watch.as_mut() {
                watch.due = Instant::now();
            }
        }
    }
    fn new() -> Self {
        let dir = tempfile::Builder::new()
            .prefix("riced-files-")
            .tempdir_in("/tmp/opencode")
            .unwrap();
        let lua = Lua::new();
        lua.globals()
            .set("riced", lua.create_table().unwrap())
            .unwrap();
        managed::install(&lua, dir.path()).unwrap();
        let app = lua.create_table().unwrap();
        managed::attach(&lua, &app).unwrap();
        lua.globals().set("app", app).unwrap();
        managed::phase(&lua, managed::Phase::Active);
        let mut owner = Owner::default();
        owner.reset(managed::generation(&lua).unwrap());
        Self { lua, dir, owner }
    }
    fn exec(&self, source: &str) {
        self.lua.load(source).exec().unwrap();
    }
    fn flag(&self, source: &str) -> bool {
        self.lua.load(source).eval().unwrap()
    }
    fn jobs(&mut self) -> Vec<crate::framework::tasks::Job> {
        self.owner.apply(managed::commands(&self.lua));
        self.owner.jobs()
    }
    async fn pump(&mut self) {
        for job in self.jobs() {
            let delivery = job.run().await;
            assert!(self.owner.complete(&delivery));
            managed::invoke(&self.lua, &delivery).unwrap();
        }
    }
}
fn run(future: impl std::future::Future<Output = ()>) {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
}

#[test]
fn defaults_are_cached_and_loaded_maps_merge_while_arrays_replace() {
    let mut f = Fixture::new();
    std::fs::write(
        f.dir.path().join("settings.json"),
        r#"{"appearance":{"opacity":0.5},"list":[9]}"#,
    )
    .unwrap();
    f.exec(r#"file=riced.file({type='json',path='settings.json',defaults={appearance={opacity=0.9,radius=8},list={1,2}}})
        assert(file:get({'appearance','opacity'})==0.9)
        local copy=file:get(); copy.appearance.opacity=99
        assert(file:get({'appearance','opacity'})==0.9)
        file:reload(function(result) assert(result.ok); loaded=true end)"#);
    run(f.pump());
    assert!(f.flag("return loaded and file:get({'appearance','opacity'})==0.5 and file:get({'appearance','radius'})==8 and #file:get('list')==1 and not file:status().dirty"));
}

#[test]
fn missing_file_defaults_are_not_written_until_an_explicit_save() {
    let mut f = Fixture::new();
    f.exec("file=riced.file({type='toml',path='settings.toml',defaults={enabled=true}})");
    assert!(!f.dir.path().join("settings.toml").exists());
    assert!(f.lua.load("file:save()").exec().is_err());
    f.exec("file:reload()");
    assert!(f.lua.load("file:save()").exec().is_err());
    run(f.pump());
    f.exec("file:save(function(result) assert(result.ok) end)");
    run(f.pump());
    assert!(
        std::fs::read_to_string(f.dir.path().join("settings.toml"))
            .unwrap()
            .contains("enabled = true")
    );
    assert!(f.flag(
        "return file:status().loaded and not file:status().dirty and not file:status().busy"
    ));
}

#[test]
fn saving_a_captured_revision_does_not_mark_newer_edits_clean() {
    let mut f = Fixture::new();
    f.exec("file=riced.file({type='json',path='settings.json'}); file:reload()");
    run(f.pump());
    f.exec("file:set('count',1); file:save(function(result) saved=result end)");
    run(async {
        let job = f.jobs().pop().unwrap();
        let delivery = job.run().await;
        f.exec("file:set('count',2)");
        assert!(f.owner.complete(&delivery));
        managed::invoke(&f.lua, &delivery).unwrap();
    });
    assert!(
        f.flag("return saved.ok and saved.dirty and file:get('count')==2 and file:status().dirty")
    );
    assert_eq!(
        storage::decode(
            Format::Json,
            &std::fs::read_to_string(f.dir.path().join("settings.json")).unwrap()
        )
        .unwrap()["count"],
        1
    );
    f.exec("file:save()");
    run(f.pump());
    assert!(!f.flag("return file:status().dirty"));
}

#[test]
fn reloads_preserve_local_edits_and_require_explicit_discard() {
    let mut f = Fixture::new();
    std::fs::write(f.dir.path().join("settings.json"), r#"{"count":1}"#).unwrap();
    f.exec("file=riced.file({type='json',path='settings.json'}); file:reload(function(result) loaded=result end); file:set('count',2)");
    run(f.pump());
    assert!(f.flag("return not loaded.ok and loaded.error.code=='dirty' and file:get('count')==2"));
    f.exec("file:reload({discard=true},function(result) loaded=result end)");
    run(f.pump());
    assert!(f.flag("return loaded.ok and file:get('count')==1 and not file:status().dirty"));
    f.exec("file:reload({discard=true},function(result) loaded=result end); file:set('count',3)");
    run(f.pump());
    assert!(f.flag("return not loaded.ok and file:get('count')==3"));
}

#[test]
fn parse_failures_and_external_edits_never_destroy_cached_or_disk_data() {
    let mut f = Fixture::new();
    let path = f.dir.path().join("settings.json");
    std::fs::write(&path, r#"{"count":1}"#).unwrap();
    f.exec("file=riced.file({type='json',path='settings.json'}); file:reload()");
    run(f.pump());
    std::fs::write(&path, "broken").unwrap();
    f.exec("file:reload(function(result) loaded=result end)");
    run(f.pump());
    assert!(f.flag("return not loaded.ok and file:get('count')==1"));
    f.exec("file:set('count',2); file:save(function(result) saved=result end)");
    run(f.pump());
    assert!(
        f.flag("return not saved.ok and saved.error.code=='conflict' and file:get('count')==2")
    );
    assert_eq!(std::fs::read_to_string(path).unwrap(), "broken");
}

#[test]
fn cancelling_releases_busy_state_and_requires_a_fresh_reload() {
    let mut f = Fixture::new();
    f.exec("file=riced.file({type='json',path='settings.json'}); token=file:reload(function() error('cancelled callback ran') end)");
    let job = f.jobs().pop().unwrap();
    f.exec("assert(app:cancel(token)); assert(not file:status().busy); assert(not file:status().loaded)");
    run(async {
        let delivery = job.run().await;
        assert!(!managed::accepts(&f.lua, &delivery));
    });
    assert!(f.lua.load("file:save()").exec().is_err());
    f.exec("file:close()");
    assert!(f.lua.load("file:get()").eval::<Value>().is_err());
}

#[test]
fn lua_writes_are_opt_in_and_generated_arrays_roundtrip() {
    let mut f = Fixture::new();
    let path = f.dir.path().join("settings.lua");
    std::fs::write(&path, "-- original comment\nreturn {width=math.floor(2.7)}").unwrap();
    f.exec("file=riced.file({type='lua',path='settings.lua'}); file:reload()");
    run(f.pump());
    assert!(f.lua.load("file:save()").exec().is_err());
    f.exec("file:close(); file=riced.file({type='lua',path='settings.lua',write_mode='generated'}); file:reload()");
    run(f.pump());
    f.exec("file:set('array',riced.array({})); file:save()");
    run(f.pump());
    let data = storage::decode(Format::Lua, &std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(data["array"], json!([]));
    assert_eq!(data["width"], 2);
}

#[test]
fn invalid_values_view_mutations_and_handle_exhaustion_fail_early() {
    let f = Fixture::new();
    f.exec("file=riced.file({type='json',path='settings.json'})");
    for source in [
        "file:set('bad',function() end)",
        "file:set('bad',0/0)",
        "file:set('bad',{[2]='hole'})",
        "file:set('bad',nil)",
        "riced.file({type='json',path='x',typo=true})",
    ] {
        assert!(f.lua.load(source).exec().is_err(), "{source}");
    }
    managed::phase(&f.lua, managed::Phase::ReadOnly);
    assert!(f.lua.load("file:set('key',1)").exec().is_err());
    assert!(
        f.lua
            .load("riced.file({type='json',path='x'})")
            .exec()
            .is_err()
    );
    managed::phase(&f.lua, managed::Phase::Active);
    assert!(
        f.lua
            .load("for i=1,16 do riced.file({type='json',path='x'}) end")
            .exec()
            .is_err()
    );
}

const WATCH: &str = r#"
    events={}
    file=riced.file({type='json',path='settings.json',defaults={count=0}})
    token=file:watch(function(event)
        events[#events+1]=event
        if event.ok then assert(event.data.count==file:get('count')) end
    end)
"#;

#[test]
fn watches_initialize_clean_cache_follow_atomic_replacements_and_coalesce_bursts() {
    let mut f = Fixture::new();
    let path = f.dir.path().join("settings.json");
    std::fs::write(&path, r#"{"count":1}"#).unwrap();
    f.exec(WATCH);
    run(f.pump());
    assert!(f.flag("return #events==1 and events[1].event=='loaded' and file:get('count')==1"));
    let replacement = f.dir.path().join("replacement");
    std::fs::write(&replacement, r#"{"count":2}"#).unwrap();
    std::fs::rename(&replacement, &path).unwrap();
    std::fs::write(&path, r#"{"count":3}"#).unwrap();
    f.due();
    run(f.pump());
    assert!(f.flag("return #events==2 and events[2].event=='changed' and file:get('count')==3 and not file:status().dirty"));
    f.due();
    run(f.pump());
    assert!(f.flag("return #events==2"));
    assert!(!managed::was_invoked(&f.lua));
}

#[test]
fn watches_have_one_scan_in_flight_and_do_not_build_a_timer_backlog() {
    let mut f = Fixture::new();
    f.exec(WATCH);
    let jobs = f.jobs();
    assert_eq!(jobs.len(), 1);
    f.due();
    assert!(f.jobs().is_empty());
    f.due();
    assert!(f.jobs().is_empty());
    run(async {
        for job in jobs {
            let delivery = job.run().await;
            assert!(f.owner.complete(&delivery));
            managed::invoke(&f.lua, &delivery).unwrap();
        }
    });
    assert!(f.jobs().is_empty());
    assert!(f.flag("return #events==1 and events[1].missing and file:get('count')==0"));
}

#[test]
fn dirty_documents_report_deletion_conflicts_without_discarding_edits_or_recreating_files() {
    let mut f = Fixture::new();
    let path = f.dir.path().join("settings.json");
    std::fs::write(&path, r#"{"count":1}"#).unwrap();
    f.exec(WATCH);
    run(f.pump());
    f.exec("file:set('count',2)");
    std::fs::remove_file(&path).unwrap();
    f.due();
    run(f.pump());
    assert!(f.flag("return #events==2 and events[2].event=='conflict' and events[2].missing and events[2].dirty and file:get('count')==2"));
    assert!(!path.exists());
    f.due();
    run(f.pump());
    assert!(f.flag("return #events==2"));
    f.exec("file:reload({discard=true})");
    run(f.pump());
    assert!(f.flag("return file:get('count')==0 and not file:status().dirty"));
    std::fs::write(&path, r#"{"count":3}"#).unwrap();
    f.due();
    run(f.pump());
    assert!(f.flag("return #events==3 and file:get('count')==3"));
}

#[test]
fn clean_deletions_restore_defaults_and_missing_directories_can_reappear() {
    let mut f = Fixture::new();
    f.exec("events={}; file=riced.file({type='json',path='nested/settings.json',defaults={count=0}}); token=file:watch(function(event) events[#events+1]=event end)");
    run(f.pump());
    assert!(f.flag("return events[1].event=='loaded' and events[1].missing"));
    std::fs::create_dir(f.dir.path().join("nested")).unwrap();
    let path = f.dir.path().join("nested/settings.json");
    std::fs::write(&path, r#"{"count":1}"#).unwrap();
    f.due();
    run(f.pump());
    assert!(f.flag("return events[2].event=='changed' and file:get('count')==1"));
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(f.dir.path().join("nested")).unwrap();
    f.due();
    run(f.pump());
    assert!(f.flag(
        "return events[3].event=='deleted' and file:get('count')==0 and not file:status().dirty"
    ));
}

#[test]
fn malformed_content_is_reported_once_and_corrections_recover_without_losing_cache() {
    let mut f = Fixture::new();
    let path = f.dir.path().join("settings.json");
    std::fs::write(&path, r#"{"count":1}"#).unwrap();
    f.exec(WATCH);
    run(f.pump());
    std::fs::write(&path, "broken").unwrap();
    f.due();
    run(f.pump());
    assert!(f.flag("return #events==2 and events[2].event=='error' and events[2].error.code=='parse' and file:get('count')==1"));
    f.due();
    run(f.pump());
    assert!(f.flag("return #events==2"));
    std::fs::write(&path, r#"{"count":2}"#).unwrap();
    f.due();
    run(f.pump());
    assert!(f.flag("return #events==3 and events[3].ok and file:get('count')==2"));
}

#[test]
fn transient_read_errors_are_coalesced_and_report_recovery() {
    let mut f = Fixture::new();
    let path = f.dir.path().join("settings.json");
    std::fs::write(&path, r#"{"count":1}"#).unwrap();
    f.exec(WATCH);
    run(f.pump());
    std::fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink("target.json", &path).unwrap();
    f.due();
    run(f.pump());
    assert!(f.flag("return #events==2 and events[2].event=='error'"));
    f.due();
    run(f.pump());
    assert!(f.flag("return #events==2"));
    std::fs::remove_file(&path).unwrap();
    std::fs::write(&path, r#"{"count":1}"#).unwrap();
    f.due();
    run(f.pump());
    assert!(f.flag("return #events==3 and events[3].event=='recovered' and events[3].ok"));
}

#[test]
fn successful_local_saves_do_not_trigger_watch_feedback() {
    let mut f = Fixture::new();
    f.exec(WATCH);
    run(f.pump());
    f.exec("file:set('count',1); file:save()");
    run(f.pump());
    f.due();
    run(f.pump());
    assert!(f.flag("return #events==1 and file:get('count')==1 and not file:status().dirty"));
    assert!(!managed::was_invoked(&f.lua));
}

#[test]
fn late_scans_cannot_overwrite_newer_explicit_reloads() {
    let mut f = Fixture::new();
    let path = f.dir.path().join("settings.json");
    std::fs::write(&path, r#"{"count":1}"#).unwrap();
    f.exec(WATCH);
    run(f.pump());
    std::fs::write(&path, r#"{"count":2}"#).unwrap();
    f.due();
    run(async {
        let job = f.jobs().pop().unwrap();
        let old = job.run().await;
        std::fs::write(&path, r#"{"count":3}"#).unwrap();
        f.exec("file:reload()");
        f.pump().await;
        assert!(f.owner.complete(&old));
        managed::invoke(&f.lua, &old).unwrap();
        assert!(!managed::was_invoked(&f.lua));
    });
    assert!(f.flag("return file:get('count')==3 and #events==1"));
    f.due();
    run(f.pump());
    assert!(f.flag("return file:get('count')==3 and #events==1"));
}

#[test]
fn cancelling_or_closing_a_watcher_releases_resources_without_invalidating_loaded_data() {
    let mut f = Fixture::new();
    f.exec(WATCH);
    run(f.pump());
    f.due();
    let job = f.jobs().pop().unwrap();
    f.exec("assert(app:cancel(token)); assert(not file:status().watching); assert(file:status().loaded)");
    assert!(f.jobs().is_empty());
    run(async {
        let delivery = job.run().await;
        assert!(!f.owner.complete(&delivery));
        assert!(!managed::accepts(&f.lua, &delivery));
    });
    f.exec("token=file:watch(function() error('closed callback ran') end)");
    let job = f.jobs().pop().unwrap();
    f.exec("file:close()");
    assert!(f.jobs().is_empty());
    run(async {
        let delivery = job.run().await;
        assert!(!managed::accepts(&f.lua, &delivery));
    });
}

#[test]
fn watchers_can_close_their_own_file_without_reentrant_borrows() {
    let mut f = Fixture::new();
    f.exec("file=riced.file({type='json',path='settings.json'}); token=file:watch(function() file:close(); closed=true end)");
    run(f.pump());
    assert!(f.flag("return closed"));
    assert!(f.jobs().is_empty());
}

#[test]
fn watcher_options_are_strict_and_views_cannot_create_subscriptions() {
    let f = Fixture::new();
    f.exec("file=riced.file({type='json',path='settings.json'})");
    for source in [
        "file:watch()",
        "file:watch({interval_ms=0},function() end)",
        "file:watch({interval_ms=1.5},function() end)",
        "file:watch({typo=true},function() end)",
    ] {
        assert!(f.lua.load(source).exec().is_err(), "{source}");
    }
    managed::phase(&f.lua, managed::Phase::ReadOnly);
    assert!(f.lua.load("file:watch(function() end)").exec().is_err());
    managed::phase(&f.lua, managed::Phase::Active);
    f.exec("file:watch(function() end)");
    assert!(f.lua.load("file:watch(function() end)").exec().is_err());
}
