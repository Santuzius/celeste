//! Custom icondata glyphs we stitch together from upstream paths.
//!
//! `icondata` ships every icon as a `pub static IconData` with raw
//! inner-SVG path data plus paint metadata. Both consumers in this
//! crate (`widgets::run_state_icon` for in-app badges and
//! `infrastructure::tray::icons` for the tray pixmaps) accept any
//! `icondata::Icon`, so a hand-rolled static slots in transparently
//! beside the upstream ones.

/// Tabler-style "cloud sync" outline. The cloud body is the
/// `TbCloudCancelOutline` outer path verbatim (its trailing
/// `a3.45 3.45 0 0 1 2.756 1.373` arc curls into the marker slot we
/// want to fill). The two cancel paths — the radius-3 circle around
/// (19,19) and the diagonal slash — are dropped, and replaced by
/// `TbRefreshOutline` scaled by 0.5 and translated to (19,19) so the
/// refresh L-arrows fit into the same marker slot. Stroke width and
/// joins inherit from the wrapping `<svg>`, so the new strokes render
/// at the same 2-px round geometry as the cloud body.
pub static CLOUD_SYNC_OUTLINE: icondata::Icon = &icondata_core::IconData {
    style: None,
    x: None,
    y: None,
    width: Some("24"),
    height: Some("24"),
    view_box: Some("0 0 24 24"),
    stroke_linecap: Some("round"),
    stroke_linejoin: Some("round"),
    stroke_width: Some("2"),
    stroke: Some("currentColor"),
    fill: Some("none"),
    data: concat!(
        // SVG path data derived from Tabler Icons (MIT, © 2020-2024 Paweł Kuna) https://tabler.io/icons
        // Cloud outline from TbCloudCancelOutline.
        r#"<path d="M12 18.004h-5.343c-2.572 -.004 -4.657 -2.011 -4.657 -4.487"#,
        r#"c0 -2.475 2.085 -4.482 4.657 -4.482c.393 -1.762 1.794 -3.2 3.675 -3.773"#,
        r#"c1.88 -.572 3.956 -.193 5.444 1c1.488 1.19 2.162 3.007 1.77 4.769h.99"#,
        r#"a3.45 3.45 0 0 1 2.756 1.373" />"#,
        // TbRefreshOutline path 1 (top arc + top-left arrow), scaled
        // 0.5 and translated to centre (19,19).
        r#"<path stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" d="M23 18.5a4.05 4.05 0 0 0 -7.75 -1m-.25 -2v2h2" />"#,
        // TbRefreshOutline path 2 (bottom arc + bottom-right arrow),
        // same transform.
        r#"<path stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" d="M15 19.5a4.05 4.05 0 0 0 7.75 1m.25 2v-2h-2" />"#,
    ),
};
