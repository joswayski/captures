//! Host-lifetime development-package guard, separate from profile election.
use captures_app::updater::PackageUse;
use std::panic::catch_unwind;

pub struct CapturesPackageUse {
    _owner: Option<PackageUse>,
}

/// No profile access, network, renderer or worker initialization. A non-null
/// owner is returned for unpackaged hosts too; null means guard failure.
#[unsafe(no_mangle)]
pub extern "C" fn captures_package_use_current_v1() -> *mut CapturesPackageUse {
    catch_unwind(|| {
        PackageUse::current().map_or(std::ptr::null_mut(), |owner| {
            Box::into_raw(Box::new(CapturesPackageUse { _owner: owner }))
        })
    })
    .unwrap_or(std::ptr::null_mut())
}

/// Release only after all host shutdown work and the application loop finish.
///
/// # Safety
/// `handle` is null or exclusively owned from current, and freed exactly once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn captures_package_use_free_v1(handle: *mut CapturesPackageUse) {
    if !handle.is_null() {
        // SAFETY: Caller transfers the owned handle exactly once.
        drop(unsafe { Box::from_raw(handle) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unpackaged_bridge_has_independent_owned_handles_and_accepts_null_free() {
        let first = captures_package_use_current_v1();
        let second = captures_package_use_current_v1();
        assert!(!first.is_null());
        assert!(!second.is_null());
        assert_ne!(first, second);
        // SAFETY: Independent owners, no other access, each freed once.
        unsafe {
            captures_package_use_free_v1(first);
            captures_package_use_free_v1(second);
            captures_package_use_free_v1(std::ptr::null_mut());
        }
    }
}
