//! End-to-end test of the "save report, then correct & re-submit from the app"
//! feature — the new capability requested for this rebuild. This mirrors exactly
//! what the GUI buttons do, but through the core APIs so it's deterministic and
//! needs no network (dry-run) or Tesseract.

use datarunner_core::api::{self, SubmitOptions};
use datarunner_core::model::{Commodity, Extraction, TerminalType};
use datarunner_core::store::{report_from, ReportState, ReportStore};

fn extraction_at(price: u32) -> Extraction {
    let mut ex = Extraction::new("ScreenShot.jpg");
    ex.terminal_name = Some("Bueno Ravine".into());
    ex.id_terminal = Some(551);
    ex.terminal_type = Some(TerminalType::Buy);
    ex.captured_at = Some(1_786_573_224);
    let mut c = Commodity::new("Diamond");
    c.id_commodity = Some(25);
    c.status = Some(7);
    c.quantity_scu = Some(525);
    c.price = Some(price);
    ex.commodities.push(c);
    ex
}

#[test]
fn submit_save_edit_resubmit_flow() {
    let opts = SubmitOptions { dry_run: true, ..Default::default() };
    let mut store = ReportStore::in_memory();

    // 1. Initial extraction is submitted (dry-run) and saved.
    let first = extraction_at(6561);
    let resp1 = api::submit(&first, &opts, None).expect("submit");
    assert!(resp1.dry_run && resp1.is_ok());
    let first_id = store.add(report_from(&first, &resp1, None)).unwrap();

    assert_eq!(store.active().len(), 1);
    assert_eq!(store.get(&first_id).unwrap().state, ReportState::DryRun);

    // 2. User opens the saved report, edits the price, and re-submits an update.
    let saved = store.get(&first_id).unwrap().clone();
    let mut edited = saved.to_extraction();
    assert_eq!(edited.commodities[0].price, Some(6561));
    edited.commodities[0].price = Some(7200); // correction

    let resp2 = api::submit(&edited, &opts, None).expect("resubmit");
    let second_id = store.add(report_from(&edited, &resp2, Some(first_id.clone()))).unwrap();

    // 3. The original is now superseded; only the corrected report is active.
    assert_eq!(store.get(&first_id).unwrap().state, ReportState::Updated);
    let active = store.active();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].id, second_id);
    assert_eq!(active[0].commodities[0].price, Some(7200));
    assert_eq!(active[0].supersedes.as_deref(), Some(first_id.as_str()));

    // 4. Full history is retained (both reports present).
    assert_eq!(store.reports().len(), 2);
}
