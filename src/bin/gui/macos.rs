//! macOS-only glue: teach winit's application delegate to ask before quitting.
//!
//! Read `quit.rs` first — it holds the reasoning and the whole decision. This
//! file is the Objective-C runtime call that gets that decision in front of
//! AppKit, and nothing else.
//!
//! # Why a runtime method add, and not a delegate of our own
//!
//! `-[NSApplication terminate:]` consults `applicationShouldTerminate:` on the
//! app delegate and honours a `NSTerminateCancel` reply. The delegate is
//! winit's (`WinitApplicationDelegate`, installed during `EventLoop::new` —
//! winit 0.30.13 `platform_impl/macos/app_state.rs:47-61`,
//! `event_loop.rs:240`), it implements only `applicationWillTerminate:`, and
//! winit asserts on the delegate being swapped out from under it. So the one
//! non-invasive move is to ADD the missing method to the class winit already
//! declared: `class_addMethod` refuses to REPLACE an existing implementation
//! and says so in its return value, which is exactly the conservative
//! behaviour wanted here — if a future winit grows its own
//! `applicationShouldTerminate:`, this bows out and discloses rather than
//! fighting it.
//!
//! # Why raw `libobjc` and not the objc2 crates
//!
//! Four symbols and one selector. The alternative is three new dependencies
//! (objc2, objc2-app-kit, objc2-foundation) on every macOS build, version-
//! locked to whatever winit happens to use, for a call this short. The GUI
//! already links a system library the same way for the crash dialog
//! (`main.rs`, `MessageBoxW` out of `user32`), so this is the house pattern
//! rather than a new one.
//!
//! # What is NOT verified here
//!
//! This machine has no Mac. Everything below type-checks and links for both
//! Darwin targets, and the decision it defers to is covered by the battery on
//! every platform — but that a real ⌘Q, a Dock quit and a log-out each reach
//! this method can only be shown on hardware. It is on the tester-callback
//! list in the README until someone reports back.

use std::ffi::c_void;

use crate::quit::{self, TerminateReply};

type Id = *mut c_void;
type Class = *mut c_void;
type Sel = *const c_void;

// `NSApplicationTerminateReply` (AppKit/NSApplication.h). NSUInteger-wide.
const NS_TERMINATE_CANCEL: usize = 0;
const NS_TERMINATE_NOW: usize = 1;

#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    fn objc_getClass(name: *const u8) -> Class;
    fn object_getClass(obj: Id) -> Class;
    fn sel_registerName(name: *const u8) -> Sel;
    fn class_addMethod(cls: Class, name: Sel, imp: *const c_void, types: *const u8) -> u8;
    /// Declared bare and transmuted at each call site: the real symbol's
    /// signature is per-message, and taking its address is the only portable
    /// way to reach it from Rust.
    fn objc_msgSend();
}

/// `[obj sel]` for a selector that takes nothing and returns an object.
///
/// # Safety
/// `obj` must be a live Objective-C object (or a class) that responds to `sel`
/// with an object-returning, argument-free message, and the caller must be on
/// a thread where sending it is legal — for everything below, the main thread.
unsafe fn msg_send_0(obj: Id, sel: Sel) -> Id {
    let send: unsafe extern "C" fn(Id, Sel) -> Id =
        unsafe { std::mem::transmute(objc_msgSend as *const ()) };
    unsafe { send(obj, sel) }
}

/// `applicationShouldTerminate:` — AppKit calls this on the main thread when
/// anything asks the app to quit, and obeys what it returns.
unsafe extern "C" fn should_terminate(_this: Id, _cmd: Sel, _sender: Id) -> usize {
    match quit::on_terminate_request() {
        TerminateReply::Now => NS_TERMINATE_NOW,
        TerminateReply::Cancel => NS_TERMINATE_CANCEL,
    }
}

/// Add `applicationShouldTerminate:` to whatever class the live `NSApp`
/// delegate belongs to. `Err` carries a sentence fit to print.
///
/// Failure is never fatal: an app that refuses to start because it could not
/// install a confirmation dialog is worse than one that quits the way every
/// pre-guard build did. The caller discloses and carries on, and the README
/// says what the untreated behaviour is.
///
/// Must be called on the main thread, after the event loop exists — i.e. from
/// the eframe creation closure, which is where `AutoShadeApp::new` runs.
pub(crate) fn install_quit_guard() -> Result<(), &'static str> {
    // SAFETY: main thread, inside the running event loop. Every pointer below
    // is either owned by the Objective-C runtime for the life of the process
    // (classes, selectors) or the app's own singleton delegate.
    unsafe {
        let ns_application = objc_getClass(c"NSApplication".as_ptr().cast());
        let app = msg_send_0(ns_application, sel_registerName(c"sharedApplication".as_ptr().cast()));
        let delegate = msg_send_0(app, sel_registerName(c"delegate".as_ptr().cast()));
        if delegate.is_null() {
            // Ran before winit installed its delegate, which would mean the
            // call has moved out of the creation closure.
            return Err("no application delegate was installed yet");
        }
        // "Q@:@" — NSUInteger return, (id self, SEL _cmd, id sender). The
        // encoding is what the runtime hands to NSInvocation-style forwarding;
        // AppKit's own call goes through objc_msgSend with the signature
        // above, so this string documents rather than dispatches.
        let added = class_addMethod(
            object_getClass(delegate),
            sel_registerName(c"applicationShouldTerminate:".as_ptr().cast()),
            should_terminate as unsafe extern "C" fn(Id, Sel, Id) -> usize as *const c_void,
            c"Q@:@".as_ptr().cast(),
        );
        if added == 0 {
            // Someone else owns the answer; do not overwrite theirs.
            return Err("the application delegate already answers applicationShouldTerminate:");
        }
    }
    Ok(())
}
