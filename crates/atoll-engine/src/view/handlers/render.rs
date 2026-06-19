use std::os::raw::c_int;

use cef::{
    AcceleratedPaintInfo, Browser, DragData, DragOperationsMask, ImplBrowser, ImplBrowserHost,
    ImplRenderHandler, MouseEvent, PaintElementType, Rect, RenderHandler, ScreenInfo,
    WrapRenderHandler, rc::Rc as _, wrap_render_handler,
};

use crate::events::EngineEvent;
use crate::registry;
use crate::render::SurfaceLayer;
use crate::view::{DragSession, ViewId, ViewRect};

wrap_render_handler! {
    pub(crate) struct ViewRenderHandler {
        view: ViewId,
    }

    impl RenderHandler {
        fn view_rect(&self, _browser: Option<&mut Browser>, rect: Option<&mut Rect>) {
            let Some(rect) = rect else { return };
            let geometry = registry::with_view(self.view, |_, view| view.geometry.get()).unwrap_or_default();
            rect.x = 0;
            rect.y = 0;
            rect.width = geometry.width.max(1);
            rect.height = geometry.height.max(1);
        }

        fn screen_info(&self, _browser: Option<&mut Browser>, screen_info: Option<&mut ScreenInfo>) -> c_int {
            let Some(screen_info) = screen_info else { return 0 };
            let Some(geometry) = registry::with_view(self.view, |_, view| view.geometry.get()) else {
                return 0;
            };
            screen_info.device_scale_factor = geometry.scale;
            1
        }

        fn on_popup_show(&self, _browser: Option<&mut Browser>, show: c_int) {
            registry::with_view(self.view, |runtime, view| {
                if show == 0 {
                    view.popup_rect.set(None);
                    view.surface.clear(SurfaceLayer::Popup);
                }
                runtime.emit(EngineEvent::PopupWidgetChanged(view.id));
            });
        }

        fn on_popup_size(&self, _browser: Option<&mut Browser>, rect: Option<&Rect>) {
            let Some(rect) = rect else { return };
            registry::with_view(self.view, |runtime, view| {
                view.popup_rect.set(Some(ViewRect {
                    x: rect.x,
                    y: rect.y,
                    width: rect.width,
                    height: rect.height,
                }));
                runtime.emit(EngineEvent::PopupWidgetChanged(view.id));
            });
        }

        fn on_paint(
            &self,
            _browser: Option<&mut Browser>,
            type_: PaintElementType,
            dirty_rects: Option<&[Rect]>,
            buffer: *const u8,
            width: c_int,
            height: c_int,
        ) {
            if buffer.is_null() || width <= 0 || height <= 0 {
                return;
            }
            let layer = layer_of(type_);
            let length = width as usize * height as usize * 4;
            // SAFETY: CEF guarantees a BGRA buffer of `width * height` pixels for the duration of
            // this callback.
            let pixels = unsafe { std::slice::from_raw_parts(buffer, length) };
            registry::with_view(self.view, |runtime, view| {
                view.surface.present_buffer(
                    &runtime.gpu,
                    layer,
                    pixels,
                    width as u32,
                    height as u32,
                    dirty_rects.unwrap_or_default(),
                );
                runtime.emit(frame_event(layer, view.id));
            });
        }

        fn on_accelerated_paint(
            &self,
            _browser: Option<&mut Browser>,
            type_: PaintElementType,
            _dirty_rects: Option<&[Rect]>,
            info: Option<&AcceleratedPaintInfo>,
        ) {
            let Some(info) = info else { return };
            let layer = layer_of(type_);
            registry::with_view(self.view, |runtime, view| {
                match view.surface.present_shared(&runtime.gpu, layer, info) {
                    Ok(()) => runtime.emit(frame_event(layer, view.id)),
                    Err(error) => {
                        if !view.paint_failed.replace(true) {
                            tracing::warn!(%error, "shared texture import failed, falling back to CPU rendering");
                            runtime.accelerated.set(false);
                            runtime.emit(EngineEvent::AcceleratedPaintFailed(view.id));
                        }
                    }
                }
            });
        }

        fn on_scroll_offset_changed(&self, _browser: Option<&mut Browser>, x: f64, y: f64) {
            registry::with_view(self.view, |_, view| view.scroll.set((x, y)));
        }

        fn start_dragging(
            &self,
            browser: Option<&mut Browser>,
            drag_data: Option<&mut DragData>,
            allowed_ops: DragOperationsMask,
            x: c_int,
            y: c_int,
        ) -> c_int {
            let (Some(browser), Some(drag_data)) = (browser, drag_data) else { return 0 };
            let Some(host) = browser.host() else { return 0 };
            let started = registry::with_view(self.view, |_, view| {
                let event = MouseEvent {
                    x,
                    y,
                    modifiers: view.pressed_buttons.get(),
                };
                host.drag_target_drag_enter(Some(drag_data), Some(&event), allowed_ops);
                *view.drag.borrow_mut() = Some(DragSession {
                    allowed_operations: allowed_ops,
                    current_operation: DragOperationsMask::default(),
                });
            });
            i32::from(started.is_some())
        }

        fn update_drag_cursor(&self, _browser: Option<&mut Browser>, operation: DragOperationsMask) {
            registry::with_view(self.view, |_, view| {
                if let Some(drag) = view.drag.borrow_mut().as_mut() {
                    drag.current_operation = operation;
                }
            });
        }
    }
}

fn layer_of(kind: PaintElementType) -> SurfaceLayer {
    if kind == PaintElementType::POPUP {
        SurfaceLayer::Popup
    } else {
        SurfaceLayer::Page
    }
}

fn frame_event(layer: SurfaceLayer, view: ViewId) -> EngineEvent {
    match layer {
        SurfaceLayer::Page => EngineEvent::FrameReady(view),
        SurfaceLayer::Popup => EngineEvent::PopupWidgetChanged(view),
    }
}
