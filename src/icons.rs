//! icondata helpers: the SVG document builder shared by the in-app
//! icons (`widgets::icon`) and the tray pixmaps
//! (`infrastructure::tray::icons`), plus custom glyphs stitched together
//! from upstream paths.
//!
//! `icondata` ships every icon as a `pub static IconData` with raw
//! inner-SVG path data plus paint metadata, so a hand-rolled static
//! slots in transparently beside the upstream ones.

/// Wrap an icon's inner path data in a complete `<svg>` document painted
/// in `color`. The icon's own paint metadata is respected: Tabler outline
/// icons declare `fill="none"` and rely on `stroke="currentColor"` plus a
/// 2-px round stroke, while fill-based packs ship their geometry inside
/// the path data and only need a flat fill.
pub fn svg_document(icon: icondata::Icon, color: &str) -> String {
    let view_box = icon.view_box.unwrap_or("0 0 24 24");
    let fill = icon.fill.unwrap_or("currentColor").replace("currentColor", color);
    let stroke = icon.stroke.unwrap_or("none").replace("currentColor", color);
    let stroke_width = icon.stroke_width.unwrap_or("1");
    let stroke_linecap = icon.stroke_linecap.unwrap_or("butt");
    let stroke_linejoin = icon.stroke_linejoin.unwrap_or("miter");
    let data = icon.data;
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="{view_box}" fill="{fill}" stroke="{stroke}" stroke-width="{stroke_width}" stroke-linecap="{stroke_linecap}" stroke-linejoin="{stroke_linejoin}">{data}</svg>"##,
    )
}

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
