//! A window shown with its own per-pixel transparency (the floating
//! button): Windows' layered windows. Elsewhere `present` never takes
//! the window over, and it is shown the ordinary way, square.

#[cfg(windows)]
pub use native_term_win::layered::Layered;

#[cfg(unix)]
mod unix {
    /// Nothing here can show a window with its own transparency.
    pub struct Layered {
        _private: (),
    }

    impl Layered {
        pub fn take_over(_handle: isize) -> Layered {
            Layered { _private: () }
        }

        pub fn is_layered(_handle: isize) -> bool {
            false
        }

        /// Always `false`: the caller shows the window the ordinary way.
        pub fn present(
            &mut self,
            _handle: isize,
            _width: u32,
            _height: u32,
            _draw: impl FnOnce(&mut [[u8; 4]]),
        ) -> bool {
            false
        }
    }
}

#[cfg(unix)]
pub use unix::Layered;
