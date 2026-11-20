//! Runnable example: classify a small synthetic measurement dataset and print
//! a byte-exact `report.json` payload to stdout (this is a report, not JSONL).

use paradl_throttle::classifier::{classify_dataset, compute_next_best_hours};
use paradl_throttle::models::{
    MeasurementDataset, SampleRecord, ThrottleReport, TimeVolumeUploadResult,
};
use paradl_throttle::report::{generate_markdown_report, generate_report_json};

fn main() {
    let samples: Vec<SampleRecord> = [9, 12, 15, 21]
        .into_iter()
        .map(|hour| SampleRecord {
            hour,
            is_weekend: false,
            source: "active".to_owned(),
            download_bps: 5_000_000.0,
            ..SampleRecord::default()
        })
        .collect();

    println!("next_best_hours (from samples):");
    for line in compute_next_best_hours(&samples) {
        println!("  - {line}");
    }

    let dataset = MeasurementDataset {
        m8_time_volume_upload: TimeVolumeUploadResult {
            samples: samples.clone(),
            ..TimeVolumeUploadResult::default()
        },
        ..MeasurementDataset::default()
    };
    let classification = classify_dataset(&dataset);
    println!(
        "verdicts: {}; time-of-day status: {}",
        classification.verdicts.len(),
        classification.time_of_day_status
    );

    let report = ThrottleReport {
        classification,
        ..ThrottleReport::default()
    };
    let json = generate_report_json(&report);
    println!("report.json bytes: {}", json.len());
    let markdown = generate_markdown_report(&report);
    println!("report.md lines: {}", markdown.lines().count());
}
