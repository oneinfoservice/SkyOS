use crate::Test;

/// Replicated PS/2 mouse packet decoder (mirrors kernel's feed_byte logic).
struct MouseState {
    x: i32, y: i32, buttons: u8, scroll: i8,
}

fn decode_packet(bytes: &[u8], has_wheel: bool) -> Option<MouseState> {
    if bytes.len() < 3 { return None; }
    let flags = bytes[0];
    if flags & 0x08 == 0 { return None; }
    if flags & 0xC0 != 0 { return None; } // overflow

    let x_rel = {
        let raw = bytes[1] as i32;
        if (flags >> 4) & 1 == 1 { raw - 256 } else { raw }
    };
    let y_rel = {
        let raw = bytes[2] as i32;
        if (flags >> 5) & 1 == 1 { raw - 256 } else { raw }
    };

    let scroll = if has_wheel && bytes.len() >= 4 {
        bytes[3] as i8
    } else { 0 };

    Some(MouseState {
        x: x_rel,
        y: y_rel,
        buttons: flags & 0x07,
        scroll,
    })
}

pub fn tests() -> Vec<Test> {
    vec![
        Test {
            name: "mouse_standard_packet_left_click",
            category: "kernel::mouse",
            run: Box::new(|| {
                // 3-byte packet: flags=0x09 (bit3=1, bit0=1 left btn), dx=10, dy=5
                let pkt = decode_packet(&[0x09, 10, 5], false).ok_or("no decode")?;
                assert_eq_result!(pkt.buttons, 1);
                assert_eq_result!(pkt.x, 10);
                assert_eq_result!(pkt.y, 5);
                assert_eq_result!(pkt.scroll, 0);
                Ok(())
            }),
        },
        Test {
            name: "mouse_standard_packet_right_click",
            category: "kernel::mouse",
            run: Box::new(|| {
                let pkt = decode_packet(&[0x0A, 0, 0], false).ok_or("no decode")?;
                assert_eq_result!(pkt.buttons, 2);
                Ok(())
            }),
        },
        Test {
            name: "mouse_standard_packet_middle_click",
            category: "kernel::mouse",
            run: Box::new(|| {
                let pkt = decode_packet(&[0x0C, 0, 0], false).ok_or("no decode")?;
                assert_eq_result!(pkt.buttons, 4);
                Ok(())
            }),
        },
        Test {
            name: "mouse_standard_packet_no_buttons",
            category: "kernel::mouse",
            run: Box::new(|| {
                let pkt = decode_packet(&[0x08, 0, 0], false).ok_or("no decode")?;
                assert_eq_result!(pkt.buttons, 0);
                Ok(())
            }),
        },
        Test {
            name: "mouse_negative_movement_x",
            category: "kernel::mouse",
            run: Box::new(|| {
                // dx=-3 (signed byte), flags bit4=1
                let pkt = decode_packet(&[0x18, 253, 0], false).ok_or("no decode")?;
                assert_eq_result!(pkt.x, -3);
                Ok(())
            }),
        },
        Test {
            name: "mouse_negative_movement_y",
            category: "kernel::mouse",
            run: Box::new(|| {
                // dy=-5 (signed byte), flags bit5=1
                let pkt = decode_packet(&[0x28, 0, 251], false).ok_or("no decode")?;
                assert_eq_result!(pkt.y, -5);
                Ok(())
            }),
        },
        Test {
            name: "mouse_scroll_wheel_up",
            category: "kernel::mouse",
            run: Box::new(|| {
                // 4-byte packet with scroll +1
                let pkt = decode_packet(&[0x08, 0, 0, 1], true).ok_or("no decode")?;
                assert_eq_result!(pkt.scroll, 1);
                Ok(())
            }),
        },
        Test {
            name: "mouse_scroll_wheel_down",
            category: "kernel::mouse",
            run: Box::new(|| {
                // 4-byte packet with scroll -1
                let pkt = decode_packet(&[0x08, 0, 0, 0xFF], true).ok_or("no decode")?;
                assert_eq_result!(pkt.scroll, -1);
                Ok(())
            }),
        },
        Test {
            name: "mouse_invalid_packet_no_bit3",
            category: "kernel::mouse",
            run: Box::new(|| {
                let pkt = decode_packet(&[0x00, 0, 0], false);
                assert_result!(pkt.is_none(), "should reject packet without bit 3");
                Ok(())
            }),
        },
        Test {
            name: "mouse_invalid_packet_overflow",
            category: "kernel::mouse",
            run: Box::new(|| {
                // flags bit6=1 (x overflow)
                let pkt = decode_packet(&[0x48, 0, 0], false);
                assert_result!(pkt.is_none(), "should reject overflow packet");
                Ok(())
            }),
        },
        Test {
            name: "mouse_multi_packet_sequence",
            category: "kernel::mouse",
            run: Box::new(|| {
                // Simulate a sequence of 3 packets as a user moves and clicks
                let packets = vec![
                    ([0x08, 10, 5], false, "move right+down"),
                    ([0x38, 253, 254], false, "move left+up"), // dx=-3 (bit4), dy=-2 (bit5)
                    ([0x09, 0, 0], false, "left click"),
                    ([0x08, 0, 0], false, "release"),
                ];
                for (bytes, wheel, desc) in packets {
                    let pkt = decode_packet(&bytes, wheel);
                    assert_result!(pkt.is_some(), "{}: decode failed", desc);
                }
                Ok(())
            }),
        },
    ]
}
