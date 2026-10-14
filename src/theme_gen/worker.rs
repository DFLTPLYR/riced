//! Palette work uses a joined, short-lived thread so its allocator thread
//! cache is released before completion is reported to the shell.

/// Called from a blocking task, never directly from an async executor thread.
/// The join ensures input pixel owners and worker-local buffers have dropped.
pub(crate) fn run<T: Send + 'static>(
    job: impl FnOnce() -> T + Send + 'static,
) -> Result<T, String> {
    let worker = std::thread::Builder::new()
        .name("riced-palette".into())
        .spawn(job)
        .map_err(|error| format!("cannot start palette worker: {error}"))?;
    let result = worker
        .join()
        .map_err(|_| "palette worker panicked".to_string());
    super::trim_memory();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn completion_releases_inputs_and_panics_become_errors() {
        let pixels = Arc::new(vec![0_u8; 1024 * 1024]);
        let weak = Arc::downgrade(&pixels);
        assert_eq!(
            run(move || {
                drop(pixels);
                7
            })
            .unwrap(),
            7
        );
        assert!(weak.upgrade().is_none());
        assert!(
            run(|| panic!("worker failure"))
                .unwrap_err()
                .contains("panicked")
        );
    }
}
