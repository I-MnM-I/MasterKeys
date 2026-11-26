//! The in-process core the Windows app runs, reached over its private connection.

use masterkeys_core::StateReport;
use masterkeys_core::dbus::DaemonProxy;

#[test]
fn embedded_core_reports_its_state() {
    let dir = tempfile::tempdir().unwrap();
    let conn = masterkeysd::embedded::start_with(dir.path().to_path_buf(), false).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let report: StateReport = runtime.block_on(async {
        let proxy = DaemonProxy::new(&conn).await.unwrap();
        serde_json::from_str(&proxy.get_state().await.unwrap()).unwrap()
    });
    assert_eq!(report.profile, "Default");
    assert!(report.connected);
}
