use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use tauri::AppHandle;

use crate::error::{AppError, AppResult};
use crate::model::SurfaceKind;
use crate::pty::sink::TauriSink;
use crate::session::heuristics::OutputObserver;

pub mod backpressure;
pub mod commands;
pub mod editor;
pub mod launch_env;
pub mod sink;
pub mod surface;

// 契約 §15 は `crate::pty::SpawnSpec` というパスを前提にする。実体は surface.rs に
// あるため、ここで再エクスポートしてそのパス解決を成立させる。
pub use surface::{PtySink, PtySurface, SpawnSpec, DEFAULT_COLS, DEFAULT_ROWS};

/// イベントトピックに埋め込む決定的な ID（契約 §5）。
/// 文字列表現は `SurfaceKind::as_db_str`（model.rs の `db_enum!`）を単一の情報源として使う。
pub fn surface_id(session_id: &str, kind: SurfaceKind) -> String {
    format!("{}:{}", session_id, kind.as_db_str())
}

/// `surface_id` の逆関数。**agent サーフェスのときだけ** `session_id` を返す。
///
/// **極性（`:agent` に限る）をここ 1 箇所に閉じるための純関数である。** 呼び出し側で
/// `rsplit_once(':')` を手書きすると、片方だけ直した瞬間に「nvim を閉じただけで
/// セッション側の何かが死ぬ」が戻ってくる。`session_id` そのものには `:` が
/// 含まれうる（`surface_id` は末尾に付ける）ので分解は右からである。
pub fn agent_session_id(surface_id: &str) -> Option<&str> {
    let (session_id, kind) = surface_id.rsplit_once(':')?;
    (kind == SurfaceKind::Agent.as_db_str()).then_some(session_id)
}

/// レジストリの 1 エントリ。`valid` は「この世代の sink 経由コールバックがまだ
/// 有効か」を表す。詳細は `RegistrySink` のドキュメント参照(必達 (a)(c))
struct Entry {
    valid: Arc<AtomicBool>,
    surface: Arc<PtySurface>,
}

/// `PtySurface::spawn` に渡す sink を包み、レジストリの生存管理を担うラッパー。
///
/// # なぜこれが要るか(必達 (a)(c))
///
/// `PtySurface` の `Drop` は SIGKILL によるリーク封鎖(Task 4)の要である。しかし
/// `Drop` は `Arc<PtySurface>` の強参照がレジストリのみになったときにしか発火
/// しない。素朴に「on_exit を受けたら呼び出し側がレジストリから remove する」
/// 設計では、remove を忘れれば Task 4 のリーク封鎖が無効化される。そこで
/// `on_exit` の発火そのものにレジストリからの自己削除を持たせ、「登録解除」と
/// 「Drop」を同じ 1 箇所に閉じ込める。
///
/// もう一つの理由は再 spawn ハザードである。`spawn_waiter_thread`
/// (surface.rs)は `alive.store(false, SeqCst)` を `sink.on_exit(...)` より
/// 先に実行するため、`is_alive() == false` は「on_exit 送出済み」を意味しない。
/// この窓(最大 `PTY_JOIN_DEADLINE` + reap 時間)の間に同じ surface_id で
/// 再 spawn すると、旧世代の on_exit が新世代の生存中に届きフロントを誤認させ
/// うる。`valid` フラグで世代を区別し、無効化された世代からの `on_data` /
/// `on_exit` は inner(実 sink)へ転送しない。
struct RegistrySink {
    inner: Arc<dyn PtySink>,
    surface_id: String,
    valid: Arc<AtomicBool>,
    registry: Weak<Mutex<HashMap<String, Entry>>>,
}

impl PtySink for RegistrySink {
    fn on_data(&self, surface_id: &str, base64: String, seq: u64) {
        // レジストリのロックは取らない(reader スレッドが毎チャンク呼ぶ経路に
        // ロックを持ち込むと、spawn 中のロック保持と絡んで詰まりうる)
        if !self.valid.load(Ordering::SeqCst) {
            return;
        }
        self.inner.on_data(surface_id, base64, seq);
    }

    fn on_exit(&self, surface_id: &str, exit_code: Option<i32>) {
        // 原子的に「この世代を無効化」を claim する。既に無効化済み(新しい
        // spawn が先に invalidate した、または二重発火)ならここで即 return し、
        // inner への転送もレジストリの操作も行わない
        if !self.valid.swap(false, Ordering::SeqCst) {
            return;
        }
        // `spawn_with_sink` が旧世代を無効化するのは新 spawn 成功「後」であり、
        // 旧世代の on_exit がそれより先に(まだ valid=true のまま)ここへ到達する
        // 順序も普通に起こりうる(旧 waiter は `alive.store(false)` してから
        // `sink.on_exit` を呼ぶまでに `PTY_JOIN_DEADLINE` 分の間があるため)。
        // その場合でも、レジストリに載っている「今の世代」が自分自身と別物なら
        // inner への転送そのものを止めなければならない。転送するかどうかの
        // 判定を ptr_eq の結果と同じロックの下で確定させ、「新しい世代が
        // 存命中に古い世代の pty://exit が飛ぶ」ことを防ぐ(必達 (c))
        let mut still_current = true;
        let removed = if let Some(registry) = self.registry.upgrade() {
            let mut guard = registry.lock().unwrap_or_else(|p| p.into_inner());
            match guard.get(&self.surface_id) {
                Some(entry) if Arc::ptr_eq(&entry.valid, &self.valid) => {
                    guard.remove(&self.surface_id)
                }
                Some(_) => {
                    // 新しい世代が同じ id で既に登録されている。この on_exit は
                    // 古い世代のものなので、レジストリも inner も触らない
                    still_current = false;
                    None
                }
                None => None,
            }
        } else {
            None
        };
        // レジストリのロックを離してから Drop(→ kill())させる
        drop(removed);
        if still_current {
            self.inner.on_exit(surface_id, exit_code);
        }
    }
}

/// 全 PTY サーフェスの登録簿。AppState が 1 つだけ持つ(契約 §15)。
///
/// `surfaces` を `Arc<Mutex<..>>` で持つのは、`RegistrySink`(各 surface の
/// on_exit)がレジストリへ弱参照で戻れるようにするため。`PtyManager` 自体を
/// `Arc` で包む必要は無い(公開シグネチャは契約どおり不変)。
pub struct PtyManager {
    surfaces: Arc<Mutex<HashMap<String, Entry>>>,
}

impl Default for PtyManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PtyManager {
    pub fn new() -> Self {
        Self {
            surfaces: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, Entry>> {
        self.surfaces
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn get_surface(&self, surface_id: &str) -> AppResult<Arc<PtySurface>> {
        self.lock()
            .get(surface_id)
            .map(|entry| Arc::clone(&entry.surface))
            .ok_or_else(|| AppError::NotFound(surface_id.to_string()))
    }

    /// PTY を起動し、読み取りスレッドを開始する。
    /// 出力は `pty://data/{surface_id}`、終了は `pty://exit/{surface_id}` で emit される。
    /// 同じ surface_id が生存中なら AppError::InvalidState を返す。
    pub fn spawn(&self, app: &AppHandle, spec: SpawnSpec) -> AppResult<()> {
        self.spawn_with_observer(app, spec, None)
    }

    /// M3-3 のヒューリスティック観測を伴う spawn。
    ///
    /// **`spawn` の署名を変えずに observer を運ぶための兄弟メソッドである**（契約 §15 は
    /// `spawn` を含む 7 つのシグネチャを凍結している）。`spawn_with_sink` 自体が
    /// 「§15 に無い `pub(crate)` の兄弟を足して依存を注入する」という同じ形の先例である。
    ///
    /// `observer` を渡してよいのは **agent サーフェスだけ**である（設計 §4.8）。
    /// nvim は常時 BEL を鳴らし、編集していない間は当然沈黙するので、editor サーフェスに
    /// 付けると値が壊れる。`editor.rs` が `spawn` を呼んだままなのは、その条件を
    /// **呼び出し側の構造で**満たす形である —— `spawn` の中で `surface_id` を見て
    /// 捨てる形にはしない（判断が 2 箇所に散る）。
    pub(crate) fn spawn_with_observer(
        &self,
        app: &AppHandle,
        spec: SpawnSpec,
        observer: Option<Box<dyn OutputObserver>>,
    ) -> AppResult<()> {
        self.spawn_with_sink_and_observer(TauriSink::new(app.clone()), spec, observer)
    }

    /// sink を差し替えられる内部版（observer なし）。
    /// `spawn_with_sink_and_observer` の 2 引数版で、ヒューリスティックを見ない
    /// テストがそのまま使う。**production の経路はここを通らない**
    /// （`spawn` / `spawn_with_observer` は observer 引数を明示する側へ直接入る）ので、
    /// テスト専用にしてある。
    #[cfg(test)]
    pub(crate) fn spawn_with_sink(&self, sink: Arc<dyn PtySink>, spec: SpawnSpec) -> AppResult<()> {
        self.spawn_with_sink_and_observer(sink, spec, None)
    }

    /// sink を差し替えられる内部版。テストが `AppHandle` を作らずに済むようにする。
    ///
    /// # ロックを spawn 全体で保持する理由
    ///
    /// 「生存チェック」と「登録」の間でロックを手放すと、同じ surface_id への
    /// 同時 spawn が両方チェックを通過し、後勝ちの `insert` が先勝ちの
    /// `PtySurface` を黙って drop してしまう(契約 §15 の排他が壊れ、一瞬
    /// 孤児 PTY が生まれる)。`PtySurface::spawn` は sink を同期的に呼ばない
    /// ため、ロックを保持したまま呼んでも再入デッドロックは起きない。
    ///
    /// # 旧世代の無効化を spawn 成功後に行う理由
    ///
    /// 生存チェックの時点で古い(死んだ)エントリを見つけても、ここでは
    /// まだ無効化しない。もし `PtySurface::spawn` がここで失敗すると、
    /// 無効化だけ済んで置き換えが起きない中途半端な状態になり、旧世代の
    /// on_exit が(無効化済みのため)二度と inner へ転送されなくなる
    /// (フロントに一生 exit が届かない実質的なリークになる)。新しい
    /// `PtySurface` の生成に成功してから初めて旧エントリを無効化し、
    /// レジストリを置き換える。
    pub(crate) fn spawn_with_sink_and_observer(
        &self,
        sink: Arc<dyn PtySink>,
        spec: SpawnSpec,
        observer: Option<Box<dyn OutputObserver>>,
    ) -> AppResult<()> {
        let id = spec.surface_id.clone();
        let mut guard = self.lock();
        if let Some(entry) = guard.get(&id) {
            if entry.surface.is_alive() {
                return Err(AppError::InvalidState(format!(
                    "surface already running: {id}"
                )));
            }
        }

        let valid = Arc::new(AtomicBool::new(true));
        let wrapped: Arc<dyn PtySink> = Arc::new(RegistrySink {
            inner: sink,
            surface_id: id.clone(),
            valid: Arc::clone(&valid),
            registry: Arc::downgrade(&self.surfaces),
        });
        let surface = PtySurface::spawn(spec, wrapped, observer)?;

        if let Some(old) = guard.get(&id) {
            // 旧世代がまだ on_exit を送出していなければ、ここで無効化する。
            // これにより、後から届く旧世代の on_exit は inner へ転送されない
            // (必達 (c))
            old.valid.store(false, Ordering::SeqCst);
        }
        // `on_exit` 側の `drop(removed)` と対称に、レジストリのロックを離してから
        // 旧エントリ(あれば)を drop する。旧エントリの `PtySurface` がここで
        // 最後の強参照を失うと `Drop` → `kill()` が走りうるため、ロック保持中に
        // それを起こさない
        let previous = guard.insert(id, Entry { valid, surface });
        drop(guard);
        drop(previous);
        Ok(())
    }

    pub fn write(&self, surface_id: &str, data: &[u8]) -> AppResult<()> {
        self.get_surface(surface_id)?.write(data)
    }

    pub fn resize(&self, surface_id: &str, cols: u16, rows: u16) -> AppResult<()> {
        self.get_surface(surface_id)?.resize(cols, rows)
    }

    /// フロントが消化した seq を通知する(契約 §9 バックプレッシャー)。
    pub fn ack(&self, surface_id: &str, seq: u64) -> AppResult<()> {
        self.get_surface(surface_id)?.ack(seq);
        Ok(())
    }

    /// SIGKILL 相当。完全に冪等: 既に死んでいる場合も、そもそも登録されていない
    /// surface_id でも `Ok(())` を返す(契約 §15)。
    ///
    /// レジストリからの削除自体はここでは行わない。`RegistrySink::on_exit` が
    /// (自然終了・kill 起因の終了を問わず)一元的に担う(必達 (a) のコメント参照)。
    pub fn kill(&self, surface_id: &str) -> AppResult<()> {
        let surface = {
            let guard = self.lock();
            guard
                .get(surface_id)
                .map(|entry| Arc::clone(&entry.surface))
        };
        match surface {
            Some(surface) => surface.kill(),
            None => Ok(()),
        }
    }

    pub fn is_alive(&self, surface_id: &str) -> bool {
        self.lock()
            .get(surface_id)
            .is_some_and(|entry| entry.surface.is_alive())
    }

    /// 生存中の全 surface_id。起動時正規化(M2-1)とアプリ終了処理で使う
    pub fn live_surfaces(&self) -> Vec<String> {
        self.lock()
            .iter()
            .filter(|(_, entry)| entry.surface.is_alive())
            .map(|(id, _)| id.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `surface_id` の逆関数。agent サーフェス以外は `None` になる。
    #[test]
    fn agent_session_id_accepts_only_the_agent_surface() {
        let sid = "3f2a9c1e-0000-4000-8000-000000000001";
        assert_eq!(
            agent_session_id(&surface_id(sid, SurfaceKind::Agent)),
            Some(sid)
        );
        assert_eq!(
            agent_session_id(&surface_id(sid, SurfaceKind::Editor)),
            None
        );
    }

    /// 分解できない形は `None`。ここで `Some(surface_id)` を返すと、
    /// 素の `session_id` が誤って agent サーフェスとして通ってしまう。
    #[test]
    fn agent_session_id_rejects_a_malformed_surface_id() {
        assert_eq!(agent_session_id("no-colon-here"), None);
        assert_eq!(agent_session_id("s1:"), None);
        assert_eq!(agent_session_id(""), None);
        // セッション id に `:` が含まれても、種別は最後の `:` の後ろで決まる
        assert_eq!(agent_session_id("a:b:agent"), Some("a:b"));
    }

    #[test]
    fn surface_id_joins_session_and_kind_with_colon() {
        assert_eq!(
            surface_id("3f2a9c1e-0000-4000-8000-000000000001", SurfaceKind::Agent),
            "3f2a9c1e-0000-4000-8000-000000000001:agent"
        );
        assert_eq!(
            surface_id("3f2a9c1e-0000-4000-8000-000000000001", SurfaceKind::Editor),
            "3f2a9c1e-0000-4000-8000-000000000001:editor"
        );
    }

    use crate::error::AppError;
    use crate::pty::surface::{PtySink, SpawnSpec, DEFAULT_COLS, DEFAULT_ROWS};
    use std::path::PathBuf;
    use std::sync::mpsc::{channel, Sender};
    use std::sync::Arc;
    use std::time::Duration;

    struct NullSink {
        tx: Sender<String>,
    }

    impl PtySink for NullSink {
        fn on_data(&self, _surface_id: &str, _base64: String, _seq: u64) {}
        fn on_exit(&self, surface_id: &str, _exit_code: Option<i32>) {
            let _ = self.tx.send(surface_id.to_string());
        }
    }

    fn manager_spec(surface_id: &str, program: &str, args: &[&str]) -> SpawnSpec {
        SpawnSpec {
            surface_id: surface_id.to_string(),
            program: program.to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
            cwd: PathBuf::from("/tmp"),
            env: Vec::new(),
            cols: DEFAULT_COLS,
            rows: DEFAULT_ROWS,
        }
    }

    /// 全 surface を止めて exit を待つ(テストの後始末)
    fn shutdown(manager: &PtyManager) {
        for id in manager.live_surfaces() {
            let _ = manager.kill(&id);
        }
    }

    #[test]
    fn spawn_registers_the_surface_as_alive() {
        let (tx, _rx) = channel();
        let manager = PtyManager::new();
        manager
            .spawn_with_sink(
                Arc::new(NullSink { tx }),
                manager_spec("s1:agent", "/bin/cat", &[]),
            )
            .expect("spawn");
        assert!(manager.is_alive("s1:agent"));
        shutdown(&manager);
    }

    #[test]
    fn spawn_rejects_a_second_spawn_while_alive() {
        let (tx, _rx) = channel();
        let sink = Arc::new(NullSink { tx });
        let manager = PtyManager::new();
        manager
            .spawn_with_sink(
                Arc::clone(&sink) as Arc<dyn PtySink>,
                manager_spec("s2:agent", "/bin/cat", &[]),
            )
            .expect("first spawn");
        let err = manager
            .spawn_with_sink(
                sink as Arc<dyn PtySink>,
                manager_spec("s2:agent", "/bin/cat", &[]),
            )
            .expect_err("second spawn must fail");
        assert!(matches!(err, AppError::InvalidState(_)), "actual: {err:?}");
        shutdown(&manager);
    }

    #[test]
    fn write_resize_and_ack_on_unknown_surface_return_not_found() {
        let manager = PtyManager::new();
        assert!(matches!(
            manager.write("nope:agent", b"x"),
            Err(AppError::NotFound(_))
        ));
        assert!(matches!(
            manager.resize("nope:agent", 80, 24),
            Err(AppError::NotFound(_))
        ));
        assert!(matches!(
            manager.ack("nope:agent", 1),
            Err(AppError::NotFound(_))
        ));
        assert!(!manager.is_alive("nope:agent"));
    }

    #[test]
    fn kill_on_unknown_surface_is_ok() {
        // 契約 §15: kill は冪等。M2-1 の起動時正規化が DB にしかない
        // セッションへ撃っても失敗させない
        let manager = PtyManager::new();
        manager
            .kill("nope:agent")
            .expect("kill on unknown must be Ok");
    }

    #[test]
    fn live_surfaces_tracks_running_surfaces_only() {
        let (tx, rx) = channel();
        let sink = Arc::new(NullSink { tx });
        let manager = PtyManager::new();
        manager
            .spawn_with_sink(
                Arc::clone(&sink) as Arc<dyn PtySink>,
                manager_spec("s3:agent", "/bin/cat", &[]),
            )
            .expect("spawn a");
        manager
            .spawn_with_sink(
                sink as Arc<dyn PtySink>,
                manager_spec("s4:agent", "/bin/cat", &[]),
            )
            .expect("spawn b");

        let mut live = manager.live_surfaces();
        live.sort();
        assert_eq!(live, vec!["s3:agent".to_string(), "s4:agent".to_string()]);

        manager.kill("s3:agent").expect("kill s3");
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(10)).expect("exit"),
            "s3:agent"
        );
        assert_eq!(manager.live_surfaces(), vec!["s4:agent".to_string()]);

        shutdown(&manager);
    }

    #[test]
    fn spawn_replaces_a_dead_surface_with_the_same_id() {
        let (tx, rx) = channel();
        let sink = Arc::new(NullSink { tx });
        let manager = PtyManager::new();
        manager
            .spawn_with_sink(
                Arc::clone(&sink) as Arc<dyn PtySink>,
                manager_spec("s5:agent", "/bin/echo", &["done"]),
            )
            .expect("first spawn");
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(10)).expect("exit"),
            "s5:agent"
        );
        manager
            .spawn_with_sink(
                sink as Arc<dyn PtySink>,
                manager_spec("s5:agent", "/bin/cat", &[]),
            )
            .expect("respawn after exit");
        assert!(manager.is_alive("s5:agent"));
        shutdown(&manager);
    }

    // --- 必達 (a): 登録解除の時点で registry が唯一の強参照であることを検証する ---

    #[test]
    fn kill_drops_the_registrys_last_strong_reference_to_the_surface() {
        let (tx, rx) = channel();
        let sink = Arc::new(NullSink { tx });
        let manager = PtyManager::new();
        manager
            .spawn_with_sink(
                Arc::clone(&sink) as Arc<dyn PtySink>,
                manager_spec("s6:agent", "/bin/cat", &[]),
            )
            .expect("spawn");
        let weak = {
            let guard = manager.surfaces.lock().unwrap_or_else(|p| p.into_inner());
            Arc::downgrade(&guard.get("s6:agent").expect("entry registered").surface)
        };

        manager.kill("s6:agent").expect("kill");
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(10)).expect("exit"),
            "s6:agent"
        );

        assert!(
            weak.upgrade().is_none(),
            "registry must be the sole strong owner: once on_exit fires and the entry is \
             deregistered, the PtySurface must actually drop (Task 4 のリーク封鎖はこれに \
             全面依存している)"
        );
    }

    // --- 必達 (b): PtyManager::kill の冪等性を 3 パターンで固定する ---
    // (「登録されていない surface_id」は上の kill_on_unknown_surface_is_ok が既に固定済み)

    #[test]
    fn kill_on_an_already_exited_surface_is_ok() {
        let (tx, rx) = channel();
        let sink = Arc::new(NullSink { tx });
        let manager = PtyManager::new();
        manager
            .spawn_with_sink(sink, manager_spec("s7:agent", "/bin/sh", &["-c", "exit 0"]))
            .expect("spawn");
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(10)).expect("exit"),
            "s7:agent"
        );

        manager
            .kill("s7:agent")
            .expect("kill on an already-exited surface must be Ok");
    }

    #[test]
    fn kill_called_twice_on_the_same_surface_is_ok_both_times() {
        let (tx, _rx) = channel();
        let sink = Arc::new(NullSink { tx });
        let manager = PtyManager::new();
        manager
            .spawn_with_sink(sink, manager_spec("s8:agent", "/bin/cat", &[]))
            .expect("spawn");

        manager.kill("s8:agent").expect("first kill");
        manager.kill("s8:agent").expect("second kill");
    }

    // --- 必達 (c): 同一 surface_id の再 spawn ハザード ---
    //
    // `spawn_waiter_thread`(surface.rs)は `alive.store(false, SeqCst)` を
    // `sink.on_exit(...)` より先に実行する(surface.rs 434 行 vs 458/473 行)。
    // つまり `is_alive() == false` は「on_exit 送出済み」を意味しない。この窓は
    // 最大 `PTY_JOIN_DEADLINE`(1 秒)+ reap 時間あり、この間に同じ surface_id で
    // 再 spawn すると、旧世代の on_exit が新世代の生存中に届いてしまいうる。
    // これは実プロセスの正確なタイミングでしか自然再現できず、そのようなテストは
    // このレーンが禁じるフレーク源(短命プロセスの厳密タイミング依存)そのものに
    // なる。そのため、この経路を塞ぐ `RegistrySink` の判定ロジック自体を
    // ホワイトボックスで決定論的に固定する。
    use std::sync::atomic::AtomicBool;
    use std::sync::Weak;

    struct RecordingSink(Sender<String>);
    impl PtySink for RecordingSink {
        fn on_data(&self, surface_id: &str, _base64: String, _seq: u64) {
            let _ = self.0.send(format!("data:{surface_id}"));
        }
        fn on_exit(&self, surface_id: &str, _exit_code: Option<i32>) {
            let _ = self.0.send(format!("exit:{surface_id}"));
        }
    }

    #[test]
    fn registry_sink_does_not_forward_on_exit_once_invalidated() {
        // 新しい世代への置き換えが先に valid を false にした後、旧世代の
        // on_exit が届いても inner(実 sink)へは届かないこと
        let (tx, rx) = channel();
        let valid = Arc::new(AtomicBool::new(false)); // 事前に無効化済み = 再 spawn 済みを模す
        let wrapper = RegistrySink {
            inner: Arc::new(RecordingSink(tx)),
            surface_id: "stale:agent".to_string(),
            valid,
            registry: Weak::new(),
        };
        wrapper.on_exit("stale:agent", Some(0));
        assert!(
            rx.try_recv().is_err(),
            "invalidated on_exit must not forward to the inner sink"
        );
    }

    #[test]
    fn registry_sink_does_not_forward_on_data_after_on_exit_has_fired() {
        // PTY_JOIN_DEADLINE のタイムアウト分岐でリークした reader が on_exit の
        // 後に on_data を最大 1 件送りうる(surface.rs の既知の記録)。この
        // stray on_data がフロントへ届かないことを固定する
        let (tx, rx) = channel();
        let valid = Arc::new(AtomicBool::new(true));
        let wrapper = RegistrySink {
            inner: Arc::new(RecordingSink(tx)),
            surface_id: "s:agent".to_string(),
            valid,
            registry: Weak::new(),
        };
        wrapper.on_exit("s:agent", None); // これ自身が valid を false にする
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(1))
                .expect("exit forwarded"),
            "exit:s:agent"
        );

        wrapper.on_data("s:agent", "AA==".to_string(), 1);
        assert!(
            rx.try_recv().is_err(),
            "on_data after on_exit must not forward (stray leaked-reader event)"
        );
    }

    #[test]
    fn registry_sink_does_not_evict_or_forward_for_a_newer_generation() {
        // ptr_eq チェックの弁別: 現在登録されているエントリの valid とは別物の
        // valid を持つ「古い世代を装った」RegistrySink が on_exit を発火しても、
        // レジストリに登録済みの新しい世代を巻き込んで消してはならず、かつ
        // 自身の inner(実 sink = フロントへの経路)へも転送してはならない
        // (転送してしまうと、生存中の新世代に対して偽の pty://exit がフロント
        // に届く。これが必達 (c) の本体)
        let (tx, rx) = channel();
        let sink = Arc::new(NullSink { tx });
        let manager = PtyManager::new();
        manager
            .spawn_with_sink(
                Arc::clone(&sink) as Arc<dyn PtySink>,
                manager_spec("s10:agent", "/bin/cat", &[]),
            )
            .expect("spawn the current generation");

        let (stale_tx, stale_rx) = channel();
        let stale = RegistrySink {
            inner: Arc::new(RecordingSink(stale_tx)),
            surface_id: "s10:agent".to_string(),
            valid: Arc::new(AtomicBool::new(true)), // 現在のエントリの valid とは別物
            registry: Arc::downgrade(&manager.surfaces),
        };
        stale.on_exit("s10:agent", Some(1));

        assert!(
            stale_rx.try_recv().is_err(),
            "a stale on_exit from a different generation must not forward to the inner sink, \
             even though the stale RegistrySink itself was never invalidated"
        );
        assert!(
            manager.is_alive("s10:agent"),
            "a stale on_exit from a different generation must not evict the current generation"
        );

        manager.kill("s10:agent").expect("cleanup");
        let _ = rx.recv_timeout(Duration::from_secs(10));
    }

    // --- Fix round 1, 指摘1: get_surface 経由(write/resize/ack)の Arc 逸脱を固定する ---
    //
    // `get_surface` は `Arc::clone(&entry.surface)` を返す。この一時クローンが
    // 呼び出しの間だけ生存し、確実に drop されることは `kill_drops_the_registrys_
    // last_strong_reference_to_the_surface` では検証されない(`kill` は
    // `get_surface` を経由せず、`kill` 自身が inline で `Arc::clone` している別経路
    // のため)。`write`/`resize`/`ack` を呼んだ後、レジストリだけが唯一の強参照で
    // あることを Weak で確認する。

    #[test]
    fn write_resize_and_ack_do_not_leak_a_strong_reference_to_the_surface() {
        let (tx, rx) = channel();
        let sink = Arc::new(NullSink { tx });
        let manager = PtyManager::new();
        manager
            .spawn_with_sink(
                Arc::clone(&sink) as Arc<dyn PtySink>,
                manager_spec("s12:agent", "/bin/cat", &[]),
            )
            .expect("spawn");

        manager.write("s12:agent", b"x").expect("write");
        manager.resize("s12:agent", 80, 24).expect("resize");
        manager.ack("s12:agent", 1).expect("ack");

        let weak = {
            let guard = manager.surfaces.lock().unwrap_or_else(|p| p.into_inner());
            Arc::downgrade(&guard.get("s12:agent").expect("entry registered").surface)
        };

        manager.kill("s12:agent").expect("kill");
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(10)).expect("exit"),
            "s12:agent"
        );

        assert!(
            weak.upgrade().is_none(),
            "write/resize/ack(get_surface 経由)は Arc の強参照を漏らしてはならない: \
             on_exit が発火してレジストリから登録解除された時点で、PtySurface は \
             実際に drop されなければならない(Task 4 のリーク封鎖はこれに \
             全面依存している)"
        );
    }

    // --- Fix round 1, 指摘2: PtyManager::kill の "registry にエントリが残ったまま
    // dead" という Some(dead) 分岐を固定する ---
    //
    // この状態は `alive.store(false)`(surface.rs 434 行)から `RegistrySink::on_exit`
    // によるレジストリからの削除までの間に構造的に到達するが、実プロセスの正確な
    // タイミングでしか自然再現できない(かつ短命プロセスの厳密タイミング依存は
    // このレーンが禁じるフレーク源そのものになる)。そのため、manager 経由で
    // spawn した生きた surface の Arc をレジストリ登録済みのまま複製し、
    // `manager.kill()` による自然な登録解除(on_exit 経由)を先に完了させてから、
    // 同じ(既に dead な)Arc をレジストリへ手動で再挿入することで、決定論的に
    // Some(dead) 分岐を通す。
    //
    // reviewer が示した「exit 受信後に Entry を再挿入」という記述を字面どおりに
    // 実装すると、Arc のクローンを exit 受信より前に取得する必要があり、
    // `/bin/sh -c "exit 0"` のような短命プロセスでは「on_exit がレジストリを
    // 削除する」タイミングと「テストコードがロックを取って Arc を読む」
    // タイミングの間に本物の競合が生まれてしまう(意図が逆にフレーク源になる)。
    // ここでは `/bin/cat` のような生存し続けるプロセスに対して Arc を先に
    // 複製し(この時点でレジストリはまだ生きたエントリを持っているので競合なし)、
    // その後 `manager.kill()` で自然に死なせてから再挿入することで、この競合を
    // 構造的に排除している。

    #[test]
    fn kill_on_a_registered_but_already_dead_surface_is_ok() {
        let (tx, rx) = channel();
        let sink = Arc::new(NullSink { tx });
        let manager = PtyManager::new();
        manager
            .spawn_with_sink(
                Arc::clone(&sink) as Arc<dyn PtySink>,
                manager_spec("s13:agent", "/bin/cat", &[]),
            )
            .expect("spawn");

        // まだ生きているうちに Arc を複製する(この時点ではレジストリと
        // このクローンの2つが強参照を持つが、レジストリ側は依然として本物の
        // エントリなので競合しない)
        let surface = {
            let guard = manager.surfaces.lock().unwrap_or_else(|p| p.into_inner());
            Arc::clone(&guard.get("s13:agent").expect("entry registered").surface)
        };

        manager
            .kill("s13:agent")
            .expect("first kill (registry path, still Some(alive))");
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(10)).expect("exit"),
            "s13:agent"
        );
        assert!(
            !surface.is_alive(),
            "on_exit が届いた以上、surface は死んでいるはず"
        );

        // レジストリへ「登録されているが dead」なエントリを手動で作る
        {
            let mut guard = manager.surfaces.lock().unwrap_or_else(|p| p.into_inner());
            guard.insert(
                "s13:agent".to_string(),
                Entry {
                    valid: Arc::new(AtomicBool::new(true)),
                    surface,
                },
            );
        }

        manager
            .kill("s13:agent")
            .expect("kill on a registered-but-dead surface (Some(dead)) must be Ok");
    }

    // --- 必達 (d) 隣接: spawn 失敗はレジストリを汚さず、同じ id での再試行を妨げない ---
    //
    // `PtySurface::spawn` 内部のゾンビ問題(4 箇所のエラー経路が child を wait()
    // せずに残す)自体は PtyManager からは手が届かない(child/killer は
    // `PtySurface::spawn` のローカルスコープに閉じており、呼び出し元は
    // `AppResult<Arc<Self>>` しか受け取れない)。PtyManager 層で塞げる隣接項目は
    // 「spawn 失敗後もレジストリが汚れておらず、同じ id で再試行できる」ことである
    #[test]
    fn spawn_failure_leaves_the_registry_clean_for_a_retry_with_the_same_id() {
        let (tx, _rx) = channel();
        let sink = Arc::new(NullSink { tx });
        let manager = PtyManager::new();
        let err = manager
            .spawn_with_sink(
                Arc::clone(&sink) as Arc<dyn PtySink>,
                manager_spec("s11:agent", "/nonexistent/kamux-no-such-binary", &[]),
            )
            .expect_err("spawn must fail for a missing binary");
        assert!(matches!(err, AppError::PtySpawn(_)), "actual: {err:?}");
        assert!(!manager.is_alive("s11:agent"));

        manager
            .spawn_with_sink(sink, manager_spec("s11:agent", "/bin/cat", &[]))
            .expect("retry with the same id must succeed, not InvalidState");
        assert!(manager.is_alive("s11:agent"));
        shutdown(&manager);
    }
}
