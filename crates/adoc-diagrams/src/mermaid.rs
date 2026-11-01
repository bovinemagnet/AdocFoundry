use std::time::Duration;

use merman::{
    svg::SvgPipeline, CancelReason, OperationControl, RenderError, RenderOutput, RenderRequest,
    Renderer, SvgRequest,
};

use crate::{DiagramError, DiagramProvider, DiagramRequest, DEFAULT_TIMEOUT};

/// Mermaid, rendered in process by merman, as SVG that native renderers draw completely:
/// labels are SVG text rather than HTML in `foreignObject`. A render still going after
/// `timeout` stops, as an external tool does.
#[derive(Clone, Copy, Debug)]
pub struct MermaidProvider {
    timeout: Duration,
}

impl Default for MermaidProvider {
    fn default() -> Self {
        Self::with_timeout(DEFAULT_TIMEOUT)
    }
}

impl MermaidProvider {
    #[must_use]
    pub fn with_timeout(timeout: Duration) -> Self {
        Self { timeout }
    }
}

impl DiagramProvider for MermaidProvider {
    fn kinds(&self) -> &[&str] {
        &["mermaid"]
    }

    fn render(&self, request: &DiagramRequest) -> Result<String, DiagramError> {
        let output = Renderer::new()
            .render(RenderRequest::svg(
                &request.source,
                OperationControl::new().with_deadline(self.timeout),
                SvgRequest {
                    pipeline: Some(SvgPipeline::resvg_safe()),
                    ..SvgRequest::default()
                },
            ))
            .map_err(|error| match error {
                RenderError::Cancelled(cancelled)
                    if cancelled.reason == CancelReason::DeadlineExceeded =>
                {
                    DiagramError::TimedOut {
                        program: "Mermaid".to_owned(),
                        after: self.timeout,
                    }
                }
                error => DiagramError::Failed(error.to_string()),
            })?;
        match output {
            RenderOutput::Svg(Some(svg)) => Ok(svg.svg().to_owned()),
            _ => Err(DiagramError::Failed(
                "the source holds no Mermaid diagram".to_owned(),
            )),
        }
    }
}
