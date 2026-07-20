#import <AppKit/AppKit.h>
#import <objc/runtime.h>
#include <dispatch/dispatch.h>
#include "../include/wxdragon.h"
#include <wx/frame.h>
#include <wx/weakref.h>

static char WXD_PENDING_FULLSCREEN_HIDE_KEY;

@interface WXDFrameFullscreenHideObserver : NSObject {
    wxWeakRef<wxFrame> _frame;
    __weak NSWindow* _window;
}

- (instancetype)initWithFrame:(wxFrame*)frame window:(NSWindow*)window;
- (void)cancel;

@end

@implementation WXDFrameFullscreenHideObserver

- (instancetype)initWithFrame:(wxFrame*)frame window:(NSWindow*)window
{
    self = [super init];
    if (self) {
        _frame = wxWeakRef<wxFrame>(frame);
        _window = window;
        [[NSNotificationCenter defaultCenter]
            addObserver:self
               selector:@selector(windowDidExitFullScreen:)
                   name:NSWindowDidExitFullScreenNotification
                 object:window];
    }
    return self;
}

- (void)windowDidExitFullScreen:(NSNotification*)notification
{
    (void)notification;
    [[NSNotificationCenter defaultCenter] removeObserver:self];

    // AppKit posts the notification before WindowServer has fully detached the
    // window from its managed full-screen Space. Hiding synchronously here can
    // leave that Space alive with a hidden window, which appears as a black
    // desktop in Mission Control.
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, 100 * NSEC_PER_MSEC),
                   dispatch_get_main_queue(), ^{
        WXDFrameFullscreenHideObserver* keep_alive = self;
        NSWindow* window = keep_alive->_window;
        if (!window ||
            objc_getAssociatedObject(window, &WXD_PENDING_FULLSCREEN_HIDE_KEY) != keep_alive) {
            return;
        }

        wxFrame* frame = keep_alive->_frame.get();
        if (frame) {
            frame->Show(false);
        }

        [keep_alive cancel];
        if (objc_getAssociatedObject(window, &WXD_PENDING_FULLSCREEN_HIDE_KEY) == keep_alive) {
            objc_setAssociatedObject(window, &WXD_PENDING_FULLSCREEN_HIDE_KEY, nil,
                                     OBJC_ASSOCIATION_RETAIN_NONATOMIC);
        }
    });
}

- (void)cancel
{
    [[NSNotificationCenter defaultCenter] removeObserver:self];
    _frame.Release();
    _window = nil;
}

- (void)dealloc
{
    [[NSNotificationCenter defaultCenter] removeObserver:self];
}

@end

static NSWindow*
wxd_GetFrameNativeWindow(wxFrame* frame)
{
    NSView* view = frame ? frame->GetHandle() : nil;
    return view ? view.window : nil;
}

static bool
wxd_IsNativeFullscreen(NSWindow* window)
{
    return window && ([window styleMask] & NSWindowStyleMaskFullScreen) != 0;
}

static void
wxd_CancelPendingFullscreenHide(NSWindow* window)
{
    if (!window)
        return;

    WXDFrameFullscreenHideObserver* observer =
        objc_getAssociatedObject(window, &WXD_PENDING_FULLSCREEN_HIDE_KEY);
    if (!observer)
        return;

    [observer cancel];
    objc_setAssociatedObject(window, &WXD_PENDING_FULLSCREEN_HIDE_KEY, nil,
                             OBJC_ASSOCIATION_RETAIN_NONATOMIC);
}

extern "C" bool
wxd_Frame_HandleMacShow(wxd_Frame_t* frame, bool show)
{
    wxFrame* wx_frame = reinterpret_cast<wxFrame*>(frame);
    NSWindow* window = wxd_GetFrameNativeWindow(wx_frame);

    if (show) {
        wxd_CancelPendingFullscreenHide(window);
        return false;
    }

    if (!wx_frame || (!wx_frame->IsFullScreen() && !wxd_IsNativeFullscreen(window)))
        return false;

    if (!window || objc_getAssociatedObject(window, &WXD_PENDING_FULLSCREEN_HIDE_KEY))
        return window != nil;

    WXDFrameFullscreenHideObserver* observer =
        [[WXDFrameFullscreenHideObserver alloc] initWithFrame:wx_frame window:window];
    objc_setAssociatedObject(window, &WXD_PENDING_FULLSCREEN_HIDE_KEY, observer,
                             OBJC_ASSOCIATION_RETAIN_NONATOMIC);

    bool exit_started;
    if (wxd_IsNativeFullscreen(window)) {
        [window toggleFullScreen:nil];
        exit_started = true;
    } else {
        exit_started = wx_frame->ShowFullScreen(false);
    }

    if (!exit_started) {
        wxd_CancelPendingFullscreenHide(window);
        return false;
    }

    return true;
}

extern "C" void
wxd_Frame_CancelPendingMacHide(wxd_Frame_t* frame)
{
    wxFrame* wx_frame = reinterpret_cast<wxFrame*>(frame);
    wxd_CancelPendingFullscreenHide(wxd_GetFrameNativeWindow(wx_frame));
}

void
wxd_Window_SetAccessibilityLabel(wxd_Window_t* window, const char* label)
{
    if (!window || !label) return;
    wxWindow* wx_window = reinterpret_cast<wxWindow*>(window);
    NSView* view = wx_window->GetHandle();
    if (view) {
        [view setAccessibilityLabel:[NSString stringWithUTF8String:label]];
    }
}

void
wxd_App_ActivateMac(void)
{
    [[NSRunningApplication currentApplication]
        activateWithOptions:NSApplicationActivateIgnoringOtherApps];
}
