use std::os::fd::AsRawFd;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use slint::Rgb8Pixel;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{EventLoopProxy, Platform, PlatformError, WindowAdapter};

use crate::framebuffer::Framebuffer;
use crate::power::{arm_wakealarm, find_wakealarm, suspend_to_mem};
use crate::touch::TouchInput;
use crate::wakeup::{self, KindleEventLoopProxy, Queue, Wakeup};
use crate::{OnWakeCallback, WakeSchedule};

// Animations get redrawn at most ~30 fps. E-ink can't keep up with anything
// faster, so quicker wakes would just waste battery.
const ANIMATION_FRAME: Duration = Duration::from_millis(33);

/// Write every pixel of `rgb_buffer` to the hardware framebuffer and do a
/// full GC16 refresh. Shared by the normal draw path and the defensive
/// re-flush `suspend_if_idle` does right before suspending - both need the
/// exact same "write everything, not just what Slint thinks changed" logic
/// (see the call sites for why).
fn flush_full_screen(
    frame_buffer: &mut Framebuffer,
    rgb_buffer: &[Rgb8Pixel],
    gray_buffer: &mut [u8],
    width: usize,
    height: usize,
    black_and_white: bool,
) {
    let gray = &mut gray_buffer[..width];
    for row in 0..height {
        let start = row * width;
        let rgb = &rgb_buffer[start..start + width];
        for (g, p) in gray.iter_mut().zip(rgb.iter()) {
            let value = ((77 * p.r as u32 + 150 * p.g as u32 + 29 * p.b as u32) >> 8) as u8;
            *g = if black_and_white {
                if value < 128 { 0x00 } else { 0xff }
            } else {
                value
            };
        }
        frame_buffer.write_line(row, 0..width, gray);
    }
    frame_buffer.refresh_full();
}

pub(crate) struct KindlePlatform {
    pub(crate) window: Rc<MinimalSoftwareWindow>,
    start: Instant,
    queue: Queue,
    wakeup: Wakeup,
    quit_flag: Arc<AtomicBool>,
    pub(crate) wake_schedule: Arc<Mutex<Option<WakeSchedule>>>,
    pub(crate) on_wake: OnWakeCallback,
    black_and_white: Arc<AtomicBool>,
}

impl KindlePlatform {
    pub(crate) fn new(
        wake_schedule: Arc<Mutex<Option<WakeSchedule>>>,
        on_wake: OnWakeCallback,
        black_and_white: Arc<AtomicBool>,
    ) -> std::io::Result<Self> {
        let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        let wakeup = wakeup::make_wakeup()?;
        Ok(Self {
            window,
            start: Instant::now(),
            queue: Arc::new(Mutex::new(Vec::new())),
            wakeup,
            quit_flag: Arc::new(AtomicBool::new(false)),
            wake_schedule,
            on_wake,
            black_and_white,
        })
    }

    /// Suspend the device to RAM once it's been idle for `stay_awake` with no
    /// pending work, then arm the wakealarm to bring it back. Returns `true`
    /// if a suspend cycle ran (the caller should restart the event loop).
    #[allow(clippy::too_many_arguments)]
    fn suspend_if_idle(
        &self,
        frame_buffer: &mut Framebuffer,
        wakealarm: Option<&Path>,
        last_interaction: &mut Instant,
        has_touch: bool,
        rgb_buffer: &[Rgb8Pixel],
        gray_buffer: &mut [u8],
        width: usize,
    ) -> bool {
        let (Some(schedule), Some(wakealarm_path)) = (
            *self.wake_schedule.lock().expect("wake schedule poisoned"),
            wakealarm,
        ) else {
            return false;
        };

        // Pending Slint timers don't block suspend: they'll just fire on
        // resume (a 1 Hz clock timer would otherwise pin the device awake).
        let nothing_pending = !self.window.has_active_animations()
            && self
                .queue
                .lock()
                .expect("event loop closure queue poisoned")
                .is_empty();
        if last_interaction.elapsed() < schedule.stay_awake || !nothing_pending {
            return false;
        }

        // If arming fails and there's no touch input to fall back on, don't
        // suspend at all - upstream always suspends on a failed arm, on the
        // assumption that touch is always available as a backup wake
        // source. This app's touch-optional patch breaks that assumption:
        // when KOReader holds the touchscreen, touch_input is None, so a
        // failed arm with no touch means zero wake sources at all. That
        // suspends the device to RAM permanently, recoverable only with a
        // hard power-button reset. Staying awake and retrying next loop is
        // worse for battery but is recoverable; suspending forever is not.
        let arm_ok = match arm_wakealarm(wakealarm_path, schedule.wake_interval) {
            Ok(()) => true,
            Err(e) => {
                log::error!(
                    "failed to arm RTC wakealarm: {e}; device may only wake on user input this cycle"
                );
                false
            }
        };
        if !arm_ok && !has_touch {
            log::error!(
                "no RTC wakealarm and no touch input - staying awake instead of suspending with no wake source"
            );
            return false;
        }

        // A stock wake-time status flash (wifi/time/battery) can draw into
        // this shared framebuffer between this app's own draw and the
        // moment it actually suspends - confirmed on-device: a faint
        // battery-percent trace and a partial refresh seam survived this
        // app's own draw even after the KOReader screensaver overlay
        // (audit/blanket module) was disabled. Re-flushing this app's last
        // known-good frame right here, as the very last thing before
        // suspend, overwrites that trace regardless of source. Costs one
        // extra full-panel flash per wake - confirmed an acceptable
        // tradeoff over an intermittent partial ghost.
        let height = frame_buffer.height as usize;
        let black_and_white = self.black_and_white.load(Ordering::Relaxed);
        flush_full_screen(
            frame_buffer,
            rgb_buffer,
            gray_buffer,
            width,
            height,
            black_and_white,
        );
        frame_buffer.wait_for_update_complete();

        // suspend_to_mem can fail with EBUSY (a wakelock held elsewhere -
        // commonly true while charging over USB, which is how this device
        // usually gets developed against). Firing on_wake anyway, as if a
        // real suspend/resume had happened, turns a failure into a ~stay_awake
        // busy loop: a full network poll and screen flash every cycle
        // instead of every wake_interval, indefinitely, with only a log
        // line as a symptom (audit finding #11). Skip the callback here -
        // nothing actually changed, there is nothing new to render - and
        // still reset last_interaction below so the retry is paced by the
        // normal stay_awake window instead of spinning with no delay.
        let suspended = match suspend_to_mem() {
            Ok(()) => true,
            Err(e) => {
                log::error!("suspend-to-RAM failed: {e}");
                false
            }
        };

        // Fire the consumer's on-wake callback (if any) before any rendering
        // this cycle, so e.g. an HTTP poll runs before the next draw shows
        // stale data.
        if suspended {
            if let Some(callback) = self.on_wake.borrow_mut().as_mut() {
                callback();
            }
        }

        // Start the fresh stay_awake window *after* on_wake returns, not
        // before. This loop's caller re-checks suspend_if_idle immediately
        // on its next iteration, before poll()/draw_if_needed ever run - so
        // if the reset happened before the callback and the callback (e.g.
        // an HTTP poll) took longer than stay_awake, last_interaction was
        // already stale by the time we got back here, and the device would
        // suspend again immediately without ever rendering what the
        // callback just fetched.
        *last_interaction = Instant::now();
        true
    }
}

impl Platform for KindlePlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.start.elapsed()
    }

    fn new_event_loop_proxy(&self) -> Option<Box<dyn EventLoopProxy>> {
        Some(Box::new(KindleEventLoopProxy {
            queue: self.queue.clone(),
            write_fd: self.wakeup.write.clone(),
            quit_flag: self.quit_flag.clone(),
        }))
    }

    fn run_event_loop(&self) -> Result<(), PlatformError> {
        let mut frame_buffer = Framebuffer::open()
            .map_err(|e| PlatformError::Other(format!("failed to open /dev/fb0: {e}")))?;

        self.window.set_size(slint::PhysicalSize::new(
            frame_buffer.width,
            frame_buffer.height,
        ));

        // Touch is optional: a co-resident reading app (e.g. KOReader) may
        // hold an exclusive EVIOCGRAB on the touch device, which SIGSTOPping
        // it does NOT release (that only pauses scheduling, not open file
        // descriptors). Rather than crash a display-only consumer, run
        // without touch input and log why.
        let mut touch_input = match TouchInput::open(frame_buffer.width, frame_buffer.height) {
            Ok(t) => Some(t),
            Err(e) => {
                log::warn!("touch input unavailable, running display-only: {e}");
                None
            }
        };

        frame_buffer.fill(0xff);
        frame_buffer.refresh_full();

        let width = frame_buffer.width as usize;
        let mut rgb_buffer = vec![Rgb8Pixel::default(); width * frame_buffer.height as usize];
        let mut gray_buffer = vec![0u8; width];

        let wakeup_read_fd = self.wakeup.read.as_raw_fd();

        // Wakealarm path is probed once. If the device doesn't expose one
        // (e.g. running on a dev host), the suspend cycle stays disabled even
        // if a schedule is configured.
        let wakealarm = find_wakealarm().ok();
        let mut last_interaction = Instant::now();

        loop {
            // A suspend cycle restarts the loop with a fresh stay-awake window.
            if self.suspend_if_idle(
                &mut frame_buffer,
                wakealarm.as_deref(),
                &mut last_interaction,
                touch_input.is_some(),
                &rgb_buffer,
                &mut gray_buffer,
                width,
            ) {
                continue;
            }

            // Wait for touch event or wakeup from application thread.
            // -1 means "wait forever," which lets the CPU go to sleep.
            let timeout_ms: libc::c_int = match (
                self.window.has_active_animations(),
                slint::platform::duration_until_next_timer_update(),
            ) {
                (true, Some(d)) => duration_to_ms(d.min(ANIMATION_FRAME)),
                (true, None) => duration_to_ms(ANIMATION_FRAME),
                (false, Some(d)) => duration_to_ms(d),
                (false, None) => -1,
            };

            // [0] - touch events file descriptor, or -1 if touch is unavailable
            //       (poll(2) ignores negative fds)
            // [1] - wakeup pipe for userland application threads
            let mut file_descriptors = [
                libc::pollfd {
                    fd: touch_input.as_ref().map_or(-1, |t| t.fd()),
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: wakeup_read_fd,
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];

            // Block until an fd has activity or the timeout expires.
            // Retry on EINTR, bail on any other error.
            // SAFETY: fds is a valid 2-element array while poll runs.
            let poll_result = unsafe {
                libc::poll(
                    file_descriptors.as_mut_ptr(),
                    file_descriptors.len() as libc::nfds_t,
                    timeout_ms,
                )
            };
            if poll_result < 0 {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() == Some(libc::EINTR) {
                    continue;
                }
                return Err(PlatformError::Other(format!("poll failed: {err}")));
            }

            // Bail if either file descriptor has died to avoid waiting forever on input
            let err_bits = libc::POLLERR | libc::POLLHUP | libc::POLLNVAL;
            if (file_descriptors[0].revents | file_descriptors[1].revents) & err_bits != 0 {
                return Err(PlatformError::Other(format!(
                    "poll: input fd died (touch revents={:#x}, wakeup revents={:#x})",
                    file_descriptors[0].revents, file_descriptors[1].revents
                )));
            }

            // Empty the pipe before running closures so any new wakeup that arrives
            // while a closure runs still triggers another loop iteration.
            if file_descriptors[1].revents & libc::POLLIN != 0 {
                wakeup::drain(&self.wakeup.read);
                let pending: Vec<_> = self
                    .queue
                    .lock()
                    .expect("event loop closure queue poisoned")
                    .drain(..)
                    .collect();
                for c in pending {
                    c();
                }
            }

            // Check early for quit before doing more work
            if self.quit_flag.load(Ordering::SeqCst) {
                break;
            }

            // Touch activity counts as user interaction, so it resets the
            // suspend countdown
            if file_descriptors[0].revents & libc::POLLIN != 0 {
                last_interaction = Instant::now();
            }

            if let Some(t) = touch_input.as_mut() {
                t.poll(&self.window);
            }
            slint::platform::update_timers_and_animations();

            let black_and_white = self.black_and_white.load(Ordering::Relaxed);
            self.window.draw_if_needed(|renderer| {
                let dirty = renderer.render(&mut rgb_buffer, width);
                let dirty_size = dirty.bounding_box_size();

                // A degenerate dirty region means nothing visually changed.
                // The removed refresh_region() call had this same guard -
                // draw_if_needed() can still invoke this closure with a
                // zero-size region, and without this check that produced a
                // needless full-panel GC16 flash (audit finding #8).
                if dirty_size.width == 0 || dirty_size.height == 0 {
                    return;
                }

                // Write every pixel this app owns, not just Slint's
                // reported dirty region. KOReader's stock status bar
                // shares this same physical framebuffer and can draw
                // into it outside this app's own content - Slint's
                // dirty tracking only knows about its own scene, so a
                // corner it drew into but this app's own UI never
                // touches again would keep its stale content forever.
                // A "full" GC16 refresh redraws whatever the buffer
                // already holds, not a blank canvas, so it does not fix
                // this on its own. Always writing the whole screen
                // guarantees this app's own background overwrites
                // anything else on every single refresh (confirmed live:
                // a stale status-bar clock, six minutes behind the real
                // time, was still visible in the corner this app's own
                // UI never draws into). The extra copy cost is
                // negligible at the app's several-minute refresh
                // interval. This also does the panel's full GC16 refresh -
                // needed regardless of dirty-rect size, since a partial
                // (AUTO waveform) refresh doesn't fully reset the panel's
                // grey levels and leaves faint ghosting behind, the same
                // physical e-ink behavior seen in the shell-script version
                // of this dashboard, fixed there the same way.
                let height = frame_buffer.height as usize;
                flush_full_screen(
                    &mut frame_buffer,
                    &rgb_buffer,
                    &mut gray_buffer,
                    width,
                    height,
                    black_and_white,
                );
            });
        }

        Ok(())
    }
}

fn duration_to_ms(d: Duration) -> libc::c_int {
    // Round up to at least 1 ms. A timeout of 0 makes poll skip the wait
    // entirely, which would spin the CPU if a tiny timer kept re-firing.
    d.as_millis().clamp(1, libc::c_int::MAX as u128) as libc::c_int
}
