use thiserror::Error;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("the browser engine could not be initialized")]
    Initialization,
    #[error("the browser engine is not running")]
    NotRunning,
    #[error("the browser could not be created")]
    BrowserCreation,
}
