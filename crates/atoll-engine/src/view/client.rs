use cef::{
    Client, ContextMenuHandler, DialogHandler, DisplayHandler, DownloadHandler, FindHandler,
    ImplClient, JsdialogHandler, LifeSpanHandler, LoadHandler, RenderHandler, RequestHandler,
    WrapClient, rc::Rc as _, wrap_client,
};

use super::ViewId;
use super::handlers::{
    ViewContextMenuHandler, ViewDialogHandler, ViewDisplayHandler, ViewDownloadHandler,
    ViewFindHandler, ViewJsDialogHandler, ViewLifeSpanHandler, ViewLoadHandler, ViewRenderHandler,
    ViewRequestHandler,
};

#[derive(Clone)]
struct ViewHandlers {
    render: RenderHandler,
    display: DisplayHandler,
    life_span: LifeSpanHandler,
    load: LoadHandler,
    request: RequestHandler,
    context_menu: ContextMenuHandler,
    dialog: DialogHandler,
    jsdialog: JsdialogHandler,
    download: DownloadHandler,
    find: FindHandler,
}

wrap_client! {
    struct ViewClient {
        handlers: ViewHandlers,
    }

    impl Client {
        fn render_handler(&self) -> Option<RenderHandler> {
            Some(self.handlers.render.clone())
        }

        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(self.handlers.display.clone())
        }

        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(self.handlers.life_span.clone())
        }

        fn load_handler(&self) -> Option<LoadHandler> {
            Some(self.handlers.load.clone())
        }

        fn request_handler(&self) -> Option<RequestHandler> {
            Some(self.handlers.request.clone())
        }

        fn context_menu_handler(&self) -> Option<ContextMenuHandler> {
            Some(self.handlers.context_menu.clone())
        }

        fn dialog_handler(&self) -> Option<DialogHandler> {
            Some(self.handlers.dialog.clone())
        }

        fn jsdialog_handler(&self) -> Option<JsdialogHandler> {
            Some(self.handlers.jsdialog.clone())
        }

        fn download_handler(&self) -> Option<DownloadHandler> {
            Some(self.handlers.download.clone())
        }

        fn find_handler(&self) -> Option<FindHandler> {
            Some(self.handlers.find.clone())
        }
    }
}

/// Creates the client object that routes every callback of one browser to its view.
pub(crate) fn build_client(view: ViewId) -> Client {
    ViewClient::new(ViewHandlers {
        render: ViewRenderHandler::new(view),
        display: ViewDisplayHandler::new(view),
        life_span: ViewLifeSpanHandler::new(view),
        load: ViewLoadHandler::new(view),
        request: ViewRequestHandler::new(view),
        context_menu: ViewContextMenuHandler::new(view),
        dialog: ViewDialogHandler::new(view),
        jsdialog: ViewJsDialogHandler::new(view),
        download: ViewDownloadHandler::new(view),
        find: ViewFindHandler::new(view),
    })
}
