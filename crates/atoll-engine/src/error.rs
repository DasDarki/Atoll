use thiserror::Error;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("the browser engine could not be initialized")]
    Initialization,
    #[error("the browser engine is not running")]
    NotRunning,
    #[error("the browser could not be created")]
    BrowserCreation,
    #[error("the developer tools could not be started: {0}")]
    DevTools(std::io::Error),
}
