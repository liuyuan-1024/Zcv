use core::pin::Pin;

use futures_core::Stream;

use crate::error::AiError;
use crate::request::ChatRequest;
use crate::stream::StreamEvent;

/// provider 返回的事件流。
///
/// 用 trait object 而非 `impl Stream`，让上层可以把不同 provider 装进同一容器。
/// 取消语义统一为 **drop 这个 stream** —— 具体 provider 在 stream 的 drop 实现中
/// 关闭底层连接，本 crate 不引入第二条取消通道。
pub type EventStream = Pin<Box<dyn Stream<Item = Result<StreamEvent, AiError>> + Send>>;

#[async_trait::async_trait]
pub trait AiProvider: Send + Sync {
    /// 发起一次 chat 请求。
    ///
    /// 建连 / 鉴权阶段的失败立刻 `Err` 返回；进入 stream 之后才会出现 per-event 错误。
    async fn stream(&self, request: ChatRequest) -> Result<EventStream, AiError>;
}
