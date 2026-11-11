//! DiscoLab core: enumeração de discos, leitura SMART, análise de saúde,
//! benchmark honesto (sem cache) e varredura de superfície somente-leitura.

pub mod bench;
pub mod device;
pub mod directio;
pub mod health;
pub mod report;
pub mod smart;
pub mod surface;
pub mod usb;
pub mod util;

pub use device::{Device, DriveKind, list_devices};
pub use health::{Assessment, Finding, Severity, assess};
pub use smart::{SmartReport, read_smart};
