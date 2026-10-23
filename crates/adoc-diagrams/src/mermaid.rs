use merman::{
    svg::SvgPipeline, OperationControl, RenderOutput, RenderRequest, Renderer, SvgRequest,
};

use crate::{DiagramError, DiagramProvider, DiagramRequest};

/// Mermaid, rendered in process by merman, as SVG that native renderers draw completely:
/// labels are SVG text rather than HTML in `foreignObject`.
#[derive(Clone, Copy, Debug, Default)]
pub struct MermaidProvider;

impl DiagramProvider for MermaidProvider {
    fn kinds(&self) -> &[&str] {
        &["mermaid"]
    }

    fn render(&self, request: &DiagramRequest) -> Result<String, DiagramError> {
        let output = Renderer::new()
            .render(RenderRequest::svg(
                &request.source,
                OperationControl::new(),
                SvgRequest {
                    pipeline: Some(SvgPipeline::resvg_safe()),
                    ..SvgRequest::default()
                },
            ))
            .map_err(|error| DiagramError::Failed(error.to_string()))?;
        match output {
            RenderOutput::Svg(Some(svg)) => Ok(svg.svg().to_owned()),
            _ => Err(DiagramError::Failed(
                "the source holds no Mermaid diagram".to_owned(),
            )),
        }
    }
}
