pub(crate) mod a11y;
pub(crate) mod apps;
pub(crate) mod desktop;
pub(crate) mod input;
pub(crate) mod integration;
pub(crate) mod ipc;
pub(crate) mod launcher;
pub(crate) mod layout;
pub(crate) mod regression;
pub(crate) mod renderer;
pub(crate) mod services;
pub(crate) mod session;
pub(crate) mod stress;
pub(crate) mod terminal;
pub(crate) mod theme;
pub(crate) mod window;

pub fn run_all(desktop: &mut crate::core::desktop::Desktop) -> bool {
    let mut ok = true;
    // Pure, deterministic input/keymap tests run first.
    ok &= input::test_keymap();
    ok &= input::test_from_raw();
    ok &= input::test_session_end_gate();
    ok &= input::test_logout_protocol_from_chord();
    ok &= input::test_logout_inert_with_window_open();
    // Pure palette checks: WCAG contrast pins for both themes. Cheap and
    // panic-free, so it reports even if a later stateful test panics.
    ok &= theme::test_theme_contrast();
    ok &= a11y::test_a11y_close_button();
    ok &= a11y::test_a11y_start_menu_rows();
    ok &= a11y::test_a11y_start_menu_ring_scroll();
    ok &= a11y::test_a11y_taskbar_button();
    ok &= a11y::test_tooltip_owner_label();
    ok &= a11y::test_a11y_close_resyncs_focus();
    ok &= a11y::test_a11y_resync_exclusion();
    ok &= a11y::test_focus_validate_central();
    ok &= a11y::test_a11y_activation_dismisses_overlays();
    ok &= a11y::test_a11y_overlay_mouse_keyboard_parity();
    ok &= a11y::test_a11y_taskbar_focus_feedback();
    ok &= a11y::test_a11y_window_btn_focus_feedback();
    ok &= a11y::test_a11y_focused_target();
    ok &= a11y::test_a11y_start_menu_focus_feedback();
    ok &= a11y::test_tooltip_hardening();
    ok &= a11y::test_tooltip_role_labels();
    ok &= a11y::test_a11y_full_keyboard_loop();
    ok &= a11y::test_a11y_arrows_from_byte_stream();
    ok &= a11y::test_a11y_tray_panel();
    ok &= a11y::test_a11y_keyboard_window_open();
    ok &= layout::test_layout();
    ok &= layout::test_hit_window();
    ok &= session::test_session_end_protocol();
    // Terminal tests run first: they cover the active feature work and must
    // report even if a later (pre-existing) test panics and aborts the suite.
    ok &= terminal::test_parser_semantics();
    ok &= terminal::test_terminal_pipeline(desktop);
    ok &= terminal::test_terminal_close_kills_shell(desktop);
    ok &= terminal::test_window_close_frees_pty(desktop);
    ok &= desktop::test_window_creation(desktop);
    ok &= desktop::test_window_focus(desktop);
    ok &= desktop::test_start_menu(desktop);
    ok &= desktop::test_start_menu_clicks(desktop);
    ok &= window::test_visual_flags();
    ok &= window::test_window_state();
    ok &= window::test_window_hover();
    ok &= window::test_surface_hover();
    ok &= window::test_window_pressed();
    ok &= window::test_taskbar_pressed();
    ok &= window::test_start_menu_pressed();
    ok &= apps::test_overlay_actions(desktop);
    ok &= launcher::test_spawn(desktop);
    ok &= launcher::test_spawn_at(desktop);
    ok &= launcher::test_spawn_registers(desktop);
    ok &= renderer::test_compositor_clear();
    ok &= renderer::test_compositor_layers();
    ok &= ipc::test_service_registry();
    ok &= ipc::test_permission_manager();
    ok &= ipc::test_register_defaults();
    ok &= ipc::test_exit_class();
    ok &= ipc::test_ipc_gate_granted(desktop);
    ok &= ipc::test_ipc_gate_denied(desktop);
    ok &= ipc::test_service_wire();
    ok &= ipc::test_codec_roundtrip();
    ok &= ipc::test_frame_roundtrip();
    ok &= ipc::test_poll_empty_socket();
    ok &= ipc::test_transport_end_to_end(desktop);
    ok &= services::test_notifications(desktop);
    ok &= services::test_clipboard(desktop);
    ok &= services::test_session(desktop);
    ok &= integration::test_full_flow(desktop);
    // Regression + stress suites run last: they re-verify cross-cutting
    // behaviors (drag path, theme defaults) and churn the window/notification
    // state (100-window create/close, 50 focus flips, 1000-notification flood)
    // that the functional tests never pressure. Each test settles the desktop
    // (ticks out close animations) before counting, so they are order-safe.
    ok &= regression::run_regression_suite(desktop);
    ok &= stress::run_stress_tests(desktop);
    ok
}
