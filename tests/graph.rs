//! Graph contracts against temporary files created and updated through librrd.

use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, UNIX_EPOCH};

use indoc::indoc;
use kal_rrd_graph::graph::{build, Window};
use kal_rrd_graph::GraphFields;
use rrd::ops::{create, update};
use rrd::ConsolidationFn;

const T0: i64 = 1_699_999_800;
const STEP: i64 = 60;
static WRITES: Mutex<()> = Mutex::new(());

fn fixture(path: &Path, band_step: Option<u32>) {
    let _writes = WRITES.lock().unwrap();
    let sources = [create::DataSource::gauge(
        &create::DataSourceName::new("v").unwrap(),
        300,
        None,
        None,
    )];
    let mut archives = vec![create::Archive::new(ConsolidationFn::Avg, 0.5, 1, 2000).unwrap()];
    if let Some(step) = band_step {
        for cf in [ConsolidationFn::Min, ConsolidationFn::Max] {
            archives.push(create::Archive::new(cf, 0.5, step, 2000).unwrap());
        }
    }
    create::create(
        path,
        UNIX_EPOCH + Duration::from_secs(T0 as u64),
        Duration::from_secs(STEP as u64),
        false,
        None,
        &[],
        &sources,
        &archives,
    )
    .unwrap();
    let batches: Vec<_> = (1..=100)
        .map(|i| {
            (
                update::BatchTime::Timestamp(
                    UNIX_EPOCH + Duration::from_secs((T0 + i * STEP) as u64),
                ),
                vec![update::Datum::from(i as f64)],
            )
        })
        .collect();
    update::update_all(path, update::Options::default(), &batches).unwrap();
}

fn data(band_step: Option<u32>) -> kal_rrd_graph::graph::GraphData {
    let dir = tempfile::tempdir().unwrap();
    fixture(&dir.path().join("values.rrd"), band_step);
    let graph = GraphFields::parse(indoc! {"
        files: values.rrd
        ds: v
        min-max: true
    "})
    .unwrap()
    .build("g".into(), "G".into(), "G", Some(dir.path().into()))
    .unwrap();
    build(
        &graph,
        Window::from_step(T0 + 20 * STEP, T0 + 40 * STEP, STEP),
        None,
    )
}

#[test]
fn coarse_bands_cover_each_fine_average_interval() {
    let data = data(Some(10));
    assert_eq!(
        (data.start, data.step, data.len),
        (T0 + 21 * STEP, STEP, 20)
    );
    assert_eq!(data.notes, Vec::<String>::new());
    let series = &data.series[0];
    let expected_avg: Vec<_> = (21..=40).map(|i| Some(i as f64)).collect();
    let expected_min: Vec<_> = (21..=40)
        .map(|i| Some(((i - 1) / 10 * 10 + 1) as f64))
        .collect();
    let expected_max: Vec<_> = (21..=40)
        .map(|i| Some(((i - 1) / 10 * 10 + 10) as f64))
        .collect();
    assert_eq!(series.avg, expected_avg);
    assert_eq!(series.min.as_ref().unwrap(), &expected_min);
    assert_eq!(series.max.as_ref().unwrap(), &expected_max);
    for ((avg, min), max) in series.avg.iter().zip(&expected_min).zip(&expected_max) {
        assert!(min <= avg && avg <= max);
    }
}

#[test]
fn average_only_file_retains_average_and_reports_missing_archives() {
    let data = data(None);
    assert_eq!(
        data.notes,
        ["values.rrd: no MIN archive", "values.rrd: no MAX archive"]
    );
    let series = &data.series[0];
    assert_eq!(
        series.avg,
        (21..=40).map(|i| Some(i as f64)).collect::<Vec<_>>()
    );
    assert_eq!(series.min, Some(vec![None; 20]));
    assert_eq!(series.max, Some(vec![None; 20]));
}

#[test]
fn uniform_bands_preserve_original_values() {
    let data = data(Some(1));
    assert_eq!(data.notes, Vec::<String>::new());
    let series = &data.series[0];
    let expected: Vec<_> = (21..=40).map(|i| Some(i as f64)).collect();
    assert_eq!(series.avg, expected);
    assert_eq!(series.min, Some(expected.clone()));
    assert_eq!(series.max, Some(expected));
}
