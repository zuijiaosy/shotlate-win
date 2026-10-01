use super::*;

fn content(n: usize) -> Vec<u8> {
    (0..96).flat_map(|x| [(((n * 37 + x * 11 + (n * x) % 23) % 200 + 40) as u8) & 0xF8,
        (((n * 13 + x) % 250) as u8) & 0xF8, (((n + x * 7) % 250) as u8) & 0xF8, 255]).collect()
}
fn frame(scroll: usize, header: usize, footer: usize, caret: bool) -> RgbaImage {
    let mut data = Vec::new();
    for y in 0..100 {
        let mut row = if y < header { vec![if y % 2 == 0 { 16 } else { 32 }; 96 * 4] }
            else if y >= 100 - footer { vec![if y % 2 == 0 { 200 } else { 224 }; 96 * 4] }
            else { content(scroll + y) };
        if caret && y >= 100 - footer + 4 && y < 100 - footer + 16 {
            for x in 40..42 { row[x * 4..x * 4 + 3].fill(if scroll % 2 == 0 { 0 } else { 255 }); }
        }
        data.extend(row);
    }
    RgbaImage { width: 96, height: 100, data }
}
fn stitcher() -> ScrollStitcher { ScrollStitcher::new(60_000, 0) }

#[test] fn sticky_bars_and_output() {
    let mut s = stitcher();
    assert_eq!(s.add(frame(0, 10, 8, false), None), StitchResult::Started);
    assert_eq!(s.add(frame(30, 10, 8, false), None), StitchResult::Appended(30));
    assert_eq!(s.add(frame(30, 10, 8, false), None), StitchResult::Unchanged);
    assert_eq!(s.add(frame(70, 10, 8, false), None), StitchResult::Appended(40));
    assert_eq!(s.height(), 170);
    for y in 10..162 { assert_eq!(s.row(y), content(y), "row {y}"); }
    assert_eq!(s.image().unwrap().height, 170);
    assert_eq!(s.preview(48, 1000).unwrap().height, 85);
    assert_eq!(s.preview(48, 20).unwrap().height, 20);
}
#[test] fn tall_footer_and_blinking_caret() {
    let mut s = stitcher();
    for scroll in [0, 1, 25, 45, 70, 90] { s.add(frame(scroll, 0, 30, true), None); }
    assert_eq!(s.height(), 190);
    for y in 0..160 { assert_eq!(s.row(y), content(y), "row {y}"); }
}
#[test] fn lost_overlap_recovers_and_limit_stops() {
    let mut s = stitcher(); s.add(frame(0, 10, 8, false), None);
    assert_eq!(s.add(frame(150, 10, 8, false), None), StitchResult::NoOverlap);
    assert_eq!(s.add(frame(170, 10, 8, false), None), StitchResult::NoOverlap);
    assert_eq!(s.add(frame(60, 10, 8, false), None), StitchResult::Appended(60));
    assert_eq!(s.add(frame(100, 10, 8, false), None), StitchResult::Appended(40));
    let mut limited = ScrollStitcher::new(120, 0); limited.add(frame(0, 10, 8, false), None);
    assert_eq!(limited.add(frame(30, 10, 8, false), None), StitchResult::LimitReached);
    assert_eq!(limited.height(), 100);
}
#[test] fn backward_scroll_and_rubber_band_tail() {
    let mut s = stitcher();
    for scroll in [0, 30, 60] { s.add(frame(scroll, 10, 8, false), None); }
    let mut over = frame(66, 10, 8, false);
    over.data[86 * 96 * 4..92 * 96 * 4].fill(255);
    assert_eq!(s.add(over, None), StitchResult::Appended(6));
    assert_eq!(s.add(frame(60, 10, 8, false), None), StitchResult::ScrolledBack);
    assert_eq!(s.height(), 160);
    for y in 10..152 { assert_eq!(s.row(y), content(y)); }
}
#[test] fn both_scrollbar_edges_are_excluded() {
    let mut s = ScrollStitcher::new(60_000, 8);
    let mut a = frame(0, 0, 0, false); let mut b = frame(30, 0, 0, false);
    for y in 0..100 { for x in (0..8).chain(88..96) {
        a.data[(y * 96 + x) * 4..(y * 96 + x) * 4 + 3].fill(0);
        b.data[(y * 96 + x) * 4..(y * 96 + x) * 4 + 3].fill(255);
    } }
    s.add(a, None); assert_eq!(s.add(b, None), StitchResult::Appended(30));
}
#[test] fn fractional_offsets_and_sparse_content() {
    let smooth = |scroll: f64| {
        let data = (0..100).flat_map(|y| (0..96).flat_map(move |x| {
            let v = scroll + y as f64;
            let a = 128.0 + 60.0 * (v * 0.21 + x as f64 * 0.13).sin() + 50.0 * (v * 0.037 * (x % 7 + 1) as f64).sin();
            [a.clamp(0.0, 255.0) as u8, (255.0 - a).clamp(0.0, 255.0) as u8, 128, 255]
        })).collect();
        RgbaImage { width: 96, height: 100, data }
    };
    let mut s = stitcher(); s.add(smooth(0.0), None); let mut total = 0;
    for scroll in [7.5, 19.25, 33.5, 50.0] {
        let StitchResult::Appended(d) = s.add(smooth(scroll), None) else { panic!("did not stitch {scroll}") };
        total += d; assert!((total as f64 - scroll).abs() <= 1.0);
    }
    let sparse = |scroll: usize| RgbaImage { width: 96, height: 100,
        data: (0..100).flat_map(|y| if (scroll + y) % 15 == 0 { content(scroll + y) } else { vec![250; 384] }).collect() };
    let mut s = stitcher(); s.add(sparse(0), None);
    assert_eq!(s.add(sparse(20), None), StitchResult::Appended(20));
    assert_eq!(s.add(sparse(47), None), StitchResult::Appended(27));
}
#[test] fn auto_pause_end_and_recovery() {
    let mut a = AutoScroll::default(); assert_eq!(a.settled(false, false), AutoAction::Pause);
    a.start(); assert_eq!(a.settled(false, false), AutoAction::Continue);
    assert_eq!(a.settled(true, false), AutoAction::Continue);
    assert_eq!(a.settled(false, false), AutoAction::Continue);
    assert_eq!(a.settled(false, false), AutoAction::Finish); assert!(!a.running);
    a.start(); assert_eq!(a.settled(false, true), AutoAction::Recover(2));
    assert_eq!(a.settled(false, true), AutoAction::Recover(1));
    assert_eq!(a.settled(false, true), AutoAction::Pause); assert!(!a.running);
    a.start(); a.pause(); assert_eq!(a.settled(true, false), AutoAction::Pause);
}

#[test] fn repeating_rows_require_unambiguous_evidence() {
    let periodic = |scroll: usize| RgbaImage { width: 96, height: 100,
        data: (0..100).flat_map(|y| if y < 10 { vec![16; 384] } else { content((scroll + y) % 20) }).collect() };
    let mut s = stitcher(); s.add(periodic(0), None);
    assert_eq!(s.add(periodic(30), None), StitchResult::NoOverlap);
    assert_eq!(s.height(), 100);
    assert_eq!(s.add(periodic(30), Some(28)), StitchResult::Appended(30));
    assert_eq!(s.height(), 130);
}

#[test] fn pending_frames_do_not_count_as_page_bottom() {
    let mut a = AutoScroll::default(); a.start(); a.await_frame(10);
    for _ in 0..20 { assert_eq!(a.settled(false, false), AutoAction::Wait); }
    a.frame_processed(9); assert_eq!(a.settled(false, false), AutoAction::Wait);
    a.frame_processed(10); assert_eq!(a.settled(true, false), AutoAction::Continue);
    a.await_frame(20); a.frame_processed(19);
    assert_eq!(a.settled(false, false), AutoAction::Wait);
    a.frame_processed(21); assert_eq!(a.settled(false, false), AutoAction::Continue);
    a.await_frame(30); assert_eq!(a.settled(false, false), AutoAction::Wait);
    a.frame_processed(30); assert_eq!(a.settled(false, false), AutoAction::Finish);
}

#[test] fn no_initial_movement_is_not_a_successful_capture() {
    let mut a = AutoScroll::default(); a.start();
    assert_eq!(a.settled(false, false), AutoAction::Continue);
    assert_eq!(a.settled(false, false), AutoAction::Pause);
}

#[test] fn memory_budget_and_consuming_output() {
    assert_eq!(height_budget(0), 0);
    assert_eq!(height_budget(560), 60_000);
    assert!(height_budget(3840) * 3840 * 4 <= MAX_CAPTURE_BYTES);
    assert!(height_budget(3840) < 60_000);
    let mut s = stitcher(); s.add(frame(0, 10, 8, false), None); s.add(frame(30, 10, 8, false), None);
    let expected = s.image().unwrap(); assert_eq!(s.into_image().unwrap(), expected);
}
