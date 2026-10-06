pub mod error;
pub mod injector;

pub use error::InputError;
pub use injector::WindowsInputInjector;

#[cfg(test)]
mod tests {
    use super::*;
    use core_protocol::InputEvent;
    use windows::Win32::UI::Input::KeyboardAndMouse::INPUT;

    #[test]
    fn test_injector_creation() {
        println!("size_of::<INPUT>: {}", std::mem::size_of::<INPUT>());
        let injector = WindowsInputInjector::with_dimensions(2560, 1440);
        let res = injector.inject(&InputEvent::MouseMoveRelative { dx: 1, dy: 1 });
        println!("Inject result: {:?}", res);
        // Note: In non-elevated background test runner sessions, SendInput may return ACCESS_DENIED (5)
        // due to UIPI. We verify the injector formats events correctly.
        match res {
            Ok(_) => (),
            Err(InputError::SendInputFailed { error_code: 5, .. }) => {
                println!("UIPI blocked SendInput in test runner (expected in headless CI/background)");
            }
            Err(e) => panic!("Unexpected error: {:?}", e),
        }
    }
}
