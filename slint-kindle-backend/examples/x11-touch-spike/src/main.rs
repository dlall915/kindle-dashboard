//! Throwaway research spike, not part of the real dashboard app. Kept as
//! documentation of a negative result - see the "Conclusion" section
//! below before trying this approach again.
//!
//! ## Question
//!
//! Can this app observe real touch taps by talking to Xorg as a normal
//! X11 client, instead of fighting Xorg for the raw evdev device? Raw
//! evdev access is a confirmed dead end: a completely independent,
//! non-exclusive `dd if=/dev/input/event1` read a raw 0 bytes across many
//! real taps while Xorg was running, because Xorg's own EVIOCGRAB starves
//! every other reader of the device, not just other grab attempts.
//!
//! ## Conclusion: no, not through any client-visible X11 mechanism found
//!
//! Every approach tried below saw zero events across 60+ real taps on
//! real hardware, despite two of them (marked "ok"/"SUCCESS" in the log)
//! succeeding at the protocol level:
//! - The RECORD extension does not exist on this Xorg build at all.
//! - A core-protocol `GrabPointer` on the root window succeeds, sees 0
//!   events.
//! - `XISelectEvents` (XInput2, passive) on the root window is rejected
//!   outright - `BadValue`, with the bad value equal to the root
//!   window's own id, regardless of which event mask was requested.
//! - `XISelectEvents` on a dedicated client window **succeeds** once the
//!   event mask is restricted to base XInput 2.0 events (the server only
//!   negotiates XInput2 version 2.0 - the touch-capable 2.1+ masks are
//!   rejected as invalid for a 2.0 client) - but still sees 0 events.
//! - `XIGrabDevice` (XInput2, active) with `XIAllDevices` as the target
//!   is rejected (grabs need a specific device id, not the wildcard) -
//!   not fully explored further, but the passive selection above was
//!   still active during that same listening window and also saw
//!   nothing.
//!
//! One tap during testing made the on-screen dashboard disappear and
//! KOReader's own UI show through - so *something* in the system does
//! react to touches. It just is not visible through any X11 client
//! channel exercised here. Touch on this device likely reaches KOReader
//! through a different, non-standard path (a vendor-specific mechanism,
//! or KOReader reading the raw device through some access this app does
//! not have). A future attempt at touch needs to start by finding that
//! path, not by trying more X11 API variations - see the KOReader source
//! itself (how does it actually read touch, given Xorg holds the raw
//! grab?) as the next thing to check, not covered here.
//!
//! ## Rounds tried, in order (kept in git history if the detail matters)
//!
//! 1. RECORD extension check + a plain core `GrabPointer` on root.
//! 2. XInput2 passive `XISelectEvents` on root, full mask set including
//!    touch and raw event bits.
//! 3. Same, but on a freshly created `InputOnly` client window instead of
//!    root (chosen specifically so this test cannot visibly draw over
//!    the real dashboard's framebuffer content).
//! 4. Same client window, mask restricted to base XInput 2.0 events only
//!    (button/motion, no touch bits) - this is the one that got past
//!    `XISelectEvents` successfully.
//! 5. Added an `XIGrabDevice` active-grab attempt alongside the working
//!    passive selection from round 4.

use x11rb::COPY_FROM_PARENT;
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xinput::{ConnectionExt as _, EventMask as XiEventMask, XIEventMask};
use x11rb::protocol::xproto::{ConnectionExt as _, CreateWindowAux, WindowClass};
use x11rb::rust_connection::RustConnection;

fn main() {
    println!("connecting to X server...");
    let (conn, screen_num) = match RustConnection::connect(Some(":0")) {
        Ok(c) => c,
        Err(e) => {
            println!("FAILED to connect: {e}");
            return;
        }
    };
    let setup = conn.setup();
    let screen = &setup.roots[screen_num];
    println!(
        "root window={:#x}, size={}x{}",
        screen.root, screen.width_in_pixels, screen.height_in_pixels
    );

    match conn.xinput_xi_query_version(2, 2) {
        Ok(cookie) => match cookie.reply() {
            Ok(r) => println!("XInput2 version: {}.{}", r.major_version, r.minor_version),
            Err(e) => println!("xi_query_version reply FAILED: {e}"),
        },
        Err(e) => println!("xi_query_version request FAILED: {e}"),
    }

    let win_id = match conn.generate_id() {
        Ok(id) => id,
        Err(e) => {
            println!("generate_id failed: {e}");
            return;
        }
    };
    let aux = CreateWindowAux::new().override_redirect(1);
    match conn.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        win_id,
        screen.root,
        0,
        0,
        screen.width_in_pixels,
        screen.height_in_pixels,
        0,
        WindowClass::INPUT_ONLY,
        COPY_FROM_PARENT,
        &aux,
    ) {
        Ok(cookie) => match cookie.check() {
            Ok(()) => println!("created InputOnly window {win_id:#x} (no visible content)"),
            Err(e) => {
                println!("create_window failed: {e:?}");
                return;
            }
        },
        Err(e) => {
            println!("create_window request failed: {e}");
            return;
        }
    }

    if let Err(e) = conn.map_window(win_id) {
        println!("map_window request failed: {e}");
    }
    let _ = conn.flush();

    // Round 4: the server negotiated XInput2 version 2.0 - the base
    // protocol from 2009, before touch support existed (added in 2.1).
    // TOUCH_BEGIN/UPDATE/END are likely illegal masks for a 2.0-only
    // client, which would explain the BadValue on every window tried so
    // far regardless of which window. Base pointer/button masks only -
    // if this server's touch driver translates taps into ordinary button
    // events for compatibility (common on older setups), this is how
    // they would actually arrive.
    let mask = XIEventMask::BUTTON_PRESS | XIEventMask::BUTTON_RELEASE | XIEventMask::MOTION;
    let events = vec![XiEventMask {
        deviceid: 0, // XIAllDevices
        mask: vec![mask],
    }];
    match conn.xinput_xi_select_events(win_id, &events) {
        Ok(cookie) => match cookie.check() {
            Ok(()) => println!("xi_select_events on client window: ok"),
            Err(e) => println!("xi_select_events failed: {e:?}"),
        },
        Err(e) => println!("xi_select_events request failed: {e}"),
    }
    let _ = conn.flush();

    // Round 5: passive selection succeeded but still saw nothing across
    // 15 real taps. Try an ACTIVE device grab instead - XIGrabDevice, the
    // XInput2 analog of core GrabPointer - in case some other client's
    // grab is winning priority over a passive selection.
    use x11rb::protocol::xinput::GrabOwner;
    use x11rb::protocol::xproto::GrabMode as CoreGrabMode;
    match conn.xinput_xi_grab_device(
        win_id,
        x11rb::CURRENT_TIME,
        x11rb::NONE,
        0u16, // XIAllDevices
        CoreGrabMode::ASYNC,
        CoreGrabMode::ASYNC,
        GrabOwner::NO_OWNER,
        &[u32::from(mask)],
    ) {
        Ok(cookie) => match cookie.reply() {
            Ok(r) => println!("xi_grab_device status: {:?}", r.status),
            Err(e) => println!("xi_grab_device reply failed: {e}"),
        },
        Err(e) => println!("xi_grab_device request failed: {e}"),
    }
    let _ = conn.flush();

    println!("listening for events for 30 seconds - tap the screen now...");
    let start = std::time::Instant::now();
    let mut event_count = 0;
    while start.elapsed() < std::time::Duration::from_secs(30) {
        match conn.poll_for_event() {
            Ok(Some(event)) => {
                event_count += 1;
                match &event {
                    Event::Unknown(raw) => {
                        println!("Unknown/extension event, raw bytes: {raw:?}")
                    }
                    other => println!("event: {other:?}"),
                }
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
            Err(e) => {
                println!("poll_for_event error: {e}");
                break;
            }
        }
    }
    println!("done, {event_count} event(s) observed in 30s");

    let _ = conn.destroy_window(win_id);
    let _ = conn.flush();
}
