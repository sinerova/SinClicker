use windows::Win32::Foundation::GetLastError;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEINPUT,
};

/// Number of input events one left-click down/up pair sends.
const CLICK_EVENT_COUNT: u32 = 2;

/// Sends one left-button press (down + up) at the current pointer position.
///
/// Both events are sent in a single `SendInput` call so they reach the input
/// queue together, reducing the chance of a stuck button or interleaving.
///
/// If the call does not report success for *both* events, the button may have
/// been left pressed, so a best-effort left-button **release** is sent before
/// returning `Err(WindowsError)`. Callers must not report a successful click
/// when this returns an error.
pub fn left_mouse_down_up() -> Result<(), WindowsError> {
    let down = mouse_input(MOUSEEVENTF_LEFTDOWN);
    let up = mouse_input(MOUSEEVENTF_LEFTUP);
    let events = [down, up];

    // SAFETY: `SendInput` expects a pointer to `cbsize` many `INPUT` structs;
    // we pass the whole slice, whose size equals `size_of::<INPUT>()`.
    let sent = unsafe { SendInput(&events, core::mem::size_of::<INPUT>() as i32) };

    if sent == CLICK_EVENT_COUNT {
        Ok(())
    } else {
        // Read the error *before* the best-effort release below, so the code
        // belongs to the failed click call, not to the release attempt.
        let code = unsafe {
            // SAFETY: `GetLastError` only reads the calling thread's error code.
            GetLastError()
        }
        .0;
        // `SendInput` (per the Windows docs) returns only the number of events
        // it accepted, not *which* events. With `sent` < 2 we cannot prove the
        // down reached the queue, but if it did the matching up did not, which
        // would leave the left button pressed. Send a release as a best effort
        // to avoid a stuck button; the returned error still reports the
        // original (accurate) failure.
        let released = left_mouse_release();
        Err(WindowsError {
            code,
            message: format_failure(sent, released, code),
        })
    }
}

/// Best-effort: sends a left-button **up** so a down that was already
/// delivered (while its matching up was not) does not leave the button
/// pressed. The result is reported best-effort; it is *not* a signal that a
/// full click succeeded.
fn left_mouse_release() -> bool {
    let up = mouse_input(MOUSEEVENTF_LEFTUP);
    // SAFETY: same as `left_mouse_down_up`, a single-element `INPUT` slice.
    let sent = unsafe { SendInput(&[up], core::mem::size_of::<INPUT>() as i32) };
    sent == 1
}

/// Formats the user-facing error string from the accepted-event count, the
/// best-effort release outcome, and the `GetLastError` code recorded after the
/// call (0 when none). Only facts the API established are reported: the
/// number of events accepted and (if any) the last error code. `SendInput`
/// does not say *why* input was refused, so no cause (UIPI or otherwise) is
/// asserted.
fn format_failure(sent: u32, released: bool, code: u32) -> String {
    let error = if code != 0 {
        format!("last Windows error code {code}")
    } else {
        String::from("no Windows error code recorded")
    };
    let release = if released {
        "a best-effort left-button release was accepted"
    } else {
        "the best-effort left-button release was not accepted"
    };
    format!(
        "SendInput accepted {sent} of {CLICK_EVENT_COUNT} click events ({error}); {release}. \
         The specific cause is not reported by SendInput; Windows may refuse simulated \
         input (for example into a higher-integrity window)."
    )
}

fn mouse_input(flags: windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// A failed `SendInput` result.
#[derive(Debug)]
pub struct WindowsError {
    pub code: u32,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_message_reports_sent_count_and_release_when_no_error_recorded() {
        let message = format_failure(1, true, 0);
        assert!(message.contains("1 of 2"), "{message}");
        assert!(
            message.contains("no Windows error code recorded"),
            "{message}"
        );
        assert!(
            message.contains("best-effort left-button release was accepted"),
            "{message}"
        );
        // Must not assert a specific cause (e.g. UIPI) as the failure reason.
        assert!(!message.contains("UIPI"), "{message}");
    }

    #[test]
    fn failure_message_reports_when_release_not_accepted() {
        let message = format_failure(0, false, 0);
        assert!(message.contains("0 of 2"), "{message}");
        assert!(message.contains("was not accepted"), "{message}");
    }

    #[test]
    fn failure_message_includes_error_code_when_recorded() {
        let message = format_failure(1, true, 5);
        assert!(message.contains("last Windows error code 5"), "{message}");
        assert!(
            !message.contains("no Windows error code recorded"),
            "{message}"
        );
    }
}
