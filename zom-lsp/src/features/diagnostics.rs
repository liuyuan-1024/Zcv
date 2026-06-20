//! `textDocument/publishDiagnostics` —— 诊断信息。
//!
//! 这是服务器主动推送的通知，不是请求-响应模式。
//! 客户端收到后转发给上层（zom-desktop shell），由诊断面板消费。
