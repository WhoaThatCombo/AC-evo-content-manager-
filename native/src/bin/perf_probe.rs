//! Dev: time the building blocks of the slow routes in-process.
//!     ACECM_ROOT=... perf_probe
use std::time::Instant;

fn t<T>(name: &str, f: impl FnOnce() -> T) -> T {
    let s = Instant::now();
    let r = f();
    println!("{:>9.2} ms  {name}", s.elapsed().as_secs_f64() * 1000.0);
    r
}

fn main() {
    for round in 0..2 {
        println!("--- round {round}");
        t("config::load", acecm::config::load);
        t("profiles::load", acecm::profiles::load);
        let profs = acecm::profiles::load();
        t("serverproc::server_pids", acecm::serverproc::server_pids);
        for p in profs.iter().take(3) {
            t("serverproc::status(profile)", || acecm::serverproc::status(p));
        }
        t("procs::tcp_listen_pids(9700)", || acecm::procs::tcp_listen_pids(9700));
        t("backend::state", acecm::backend::state);
        t("serverproc::port_clashes", acecm::serverproc::port_clashes);
        t("mods::audit", acecm::mods::audit);
        t("registry::load", acecm::registry::load);
        t("trackimport::track_map", || acecm::trackimport::track_map(false));
        t("contentsync::installed_cars", acecm::contentsync::installed_cars);
        t("content::tracks", acecm::content::tracks);
        t("content::cars", acecm::content::cars);
        t("detect::find(server_exe)", || acecm::detect::find("server_exe", false));
        t("overview::overview", acecm::overview::overview);
        t("overview::state", acecm::overview::state);
        t("gamesettings::state", acecm::gamesettings::state);
        t("autoshare state", || acecm::registry::public_list(""));
        t("events snapshot(fresh)", || acecm::events::snapshot(true));
        t("drive::status", acecm::drive::status);
        t("jobs::progress", acecm::jobs::progress);
        t("compress::status", acecm::compress::status);
        t("evoshare::status", acecm::evoshare::status);
        t("thumbs::job", acecm::thumbs::job);
        t("backend::join_state", acecm::backend::join_state);
        t("serverproc::start_state", acecm::serverproc::start_state);
    }
}
