//! A template response is a recipe, not document data. Binding it to a frozen, host-owned native
//! bundle produces a complete in-memory draft. No file/network lookup or Session mutation occurs.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use deckcraft_model::{LayoutId, MasterId, MediaId, Presentation, Shape, ShapeId, ShapeKind, SlideId};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{
    ContractError, ContractResult, MAX_PARAGRAPHS, MAX_TARGET_SLIDES, MAX_TEXT_CHARS, MAX_TOTAL_TEXT_BYTES, SnapshotIdentity, TextTarget,
    decode_bounded, safe_paragraph_count, validate_key,
};
#[path = "ai_contract_limits.rs"]
mod limits;

pub const MAX_TEMPLATE_SHAPES: usize = 1024;
pub const MAX_TEMPLATE_RESOURCES: usize = 2048;
pub const MAX_DRAFT_MAPPINGS: usize = 4096;
pub const MAX_MEDIA_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_TOTAL_MEDIA_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_GROUP_DEPTH: usize = 8;
pub const MAX_IMAGE_SIDE: u32 = 4096;

/// Source addresses and draft-local addresses are different types. Neither is a destination ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ShapeOwner {
    Slide { id: SlideId },
    Master { id: MasterId },
    Layout { id: LayoutId },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SourceResource {
    Slide { id: SlideId },
    Master { id: MasterId },
    Layout { id: LayoutId },
    Media { id: MediaId },
    Shape { owner: ShapeOwner, id: ShapeId },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LocalId(pub u32);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceMapping {
    pub instance: LocalId,
    pub source: SourceResource,
    pub local: LocalId,
}

#[derive(Clone, Debug)]
struct PageGraph {
    resources: BTreeSet<SourceResource>,
    linked_slides: BTreeSet<SlideId>,
}

/// Owns actual immutable native pages, masters (including themes), layouts, style defaults and
/// embedded media bytes. Deliberately not serde-enabled: it is never provider input/output.
#[derive(Clone, Debug)]
pub struct TemplateBundle {
    version: u64,
    native: Arc<Presentation>,
    graphs: BTreeMap<SlideId, PageGraph>,
}

impl TemplateBundle {
    pub fn freeze(version: u64, native: Presentation) -> ContractResult<Arc<Self>> {
        if version == 0 {
            return Err(ContractError::Identity("template version"));
        }
        if native.slides.is_empty() || native.slides.len() > MAX_TARGET_SLIDES || native.masters.len() > 64 || native.media.len() > 256 {
            return Err(ContractError::Limit("template resources"));
        }
        // Check typed native input without constructing JSON or traversing unbounded recursion.
        limits::check(&native)?;
        if !native.slide_size.width.is_finite()
            || !native.slide_size.height.is_finite()
            || native.slide_size.width <= 0.0
            || native.slide_size.height <= 0.0
        {
            return Err(ContractError::UnsupportedTemplate);
        }
        if native.notes_master.is_some()
            || native.handout_master.is_some()
            || !native.sections.is_empty()
            || !native.custom_shows.is_empty()
            || !native.embedded_fonts.is_empty()
            || native.raw_ext.is_some()
        {
            return Err(ContractError::UnsupportedTemplate);
        }
        let mut media_ids = BTreeSet::new();
        let mut media_bytes = 0usize;
        for media in &native.media {
            if media.id.0 == 0 || !media_ids.insert(media.id) {
                return Err(ContractError::ResourceGraph);
            }
            if media.link.is_some() || media.data.is_empty() || !matches!(media.content_type.as_str(), "image/png" | "image/jpeg") {
                return Err(ContractError::UnsupportedTemplate);
            }
            media_bytes = media_bytes.checked_add(media.data.len()).ok_or(ContractError::Limit("media bytes"))?;
            if media.data.len() > MAX_MEDIA_BYTES || media_bytes > MAX_TOTAL_MEDIA_BYTES {
                return Err(ContractError::Limit("media bytes"));
            }
            let format = if media.content_type == "image/png" { image::ImageFormat::Png } else { image::ImageFormat::Jpeg };
            let reader = image::ImageReader::with_format(std::io::Cursor::new(media.data.as_slice()), format);
            let (width, height) = reader.into_dimensions().map_err(|_| ContractError::UnsupportedTemplate)?;
            if width == 0 || height == 0 || width > MAX_IMAGE_SIDE || height > MAX_IMAGE_SIDE {
                return Err(ContractError::Limit("media dimensions"));
            }
        }
        let mut layouts = BTreeMap::new();
        let mut masters = BTreeSet::new();
        let mut shape_count = 0;
        for master in &native.masters {
            if master.id.0 == 0 || !masters.insert(master.id) || master.raw_ext.is_some() || master.theme.raw_extra.is_some() {
                return Err(ContractError::ResourceGraph);
            }
            check_shapes(&master.shapes, 0, &mut shape_count)?;
            for layout in &master.layouts {
                if layout.id.0 == 0 || layouts.insert(layout.id, master.id).is_some() || layout.raw_ext.is_some() {
                    return Err(ContractError::ResourceGraph);
                }
                check_shapes(&layout.shapes, 0, &mut shape_count)?;
            }
        }
        let mut graphs = BTreeMap::new();
        for slide in &native.slides {
            if slide.id.0 == 0 || graphs.contains_key(&slide.id) {
                return Err(ContractError::ResourceGraph);
            }
            if !slide.animations.is_empty()
                || slide.transition.is_some()
                || !slide.comments.is_empty()
                || !slide.notes.text().is_empty()
                || slide.raw_ext.is_some()
            {
                return Err(ContractError::UnsupportedTemplate);
            }
            check_shapes(&slide.shapes, 0, &mut shape_count)?;
            let (master, layout) = native.layout(slide.layout).ok_or(ContractError::ResourceGraph)?;
            let mut resources = BTreeSet::from([
                SourceResource::Slide { id: slide.id },
                SourceResource::Master { id: master.id },
                SourceResource::Layout { id: layout.id },
            ]);
            shape_resources(&slide.shapes, ShapeOwner::Slide { id: slide.id }, &mut resources)?;
            shape_resources(&master.shapes, ShapeOwner::Master { id: master.id }, &mut resources)?;
            shape_resources(&layout.shapes, ShapeOwner::Layout { id: layout.id }, &mut resources)?;
            // The copied master keeps only this instance's selected layout; theme stays inline.
            let mut selected_master = master.clone();
            selected_master.layouts.retain(|l| l.id == slide.layout);
            let value = serde_json::to_value((&**slide, selected_master, &native.default_text_style)).map_err(|_| ContractError::Schema)?;
            let mut linked_slides = BTreeSet::new();
            discover_references(&value, &media_ids, &mut resources, &mut linked_slides)?;
            if resources.len() > MAX_TEMPLATE_RESOURCES {
                return Err(ContractError::Limit("template graph"));
            }
            graphs.insert(slide.id, PageGraph { resources, linked_slides });
        }
        if graphs.len() + masters.len() + layouts.len() + media_ids.len() + shape_count > MAX_TEMPLATE_RESOURCES {
            return Err(ContractError::Limit("template resources"));
        }
        let value = serde_json::to_value((&native.slides, &native.masters, &native.default_text_style)).map_err(|_| ContractError::Schema)?;
        let mut all_resources = BTreeSet::new();
        let mut all_links = BTreeSet::new();
        discover_references(&value, &media_ids, &mut all_resources, &mut all_links)?;
        if all_links.iter().any(|id| !graphs.contains_key(id)) {
            return Err(ContractError::ResourceGraph);
        }
        for graph in graphs.values() {
            if graph.linked_slides.iter().any(|id| !graphs.contains_key(id)) {
                return Err(ContractError::ResourceGraph);
            }
        }
        Ok(Arc::new(Self { version, native: Arc::new(native), graphs }))
    }

    pub fn version(&self) -> u64 {
        self.version
    }
    pub fn native(&self) -> &Presentation {
        &self.native
    }
}

fn check_shapes(shapes: &[Shape], depth: usize, total: &mut usize) -> ContractResult<()> {
    check_shape_tree(shapes, depth, total, &mut BTreeSet::new())
}

fn check_shape_tree(shapes: &[Shape], depth: usize, total: &mut usize, ids: &mut BTreeSet<ShapeId>) -> ContractResult<()> {
    if depth > MAX_GROUP_DEPTH {
        return Err(ContractError::Limit("group depth"));
    }
    for shape in shapes {
        *total = total.checked_add(1).ok_or(ContractError::Limit("template shapes"))?;
        if *total > MAX_TEMPLATE_SHAPES {
            return Err(ContractError::Limit("template shapes"));
        }
        if shape.id.0 == 0 || !ids.insert(shape.id) {
            return Err(ContractError::ResourceGraph);
        }
        if shape.raw_ext.is_some() {
            return Err(ContractError::UnsupportedTemplate);
        }
        match &shape.kind {
            ShapeKind::Shape | ShapeKind::Picture { .. } => {}
            ShapeKind::Group { children, .. } => check_shape_tree(children, depth + 1, total, ids)?,
            // No opaque reference-bearing payload or executable action is admitted.
            _ => return Err(ContractError::UnsupportedTemplate),
        }
    }
    Ok(())
}

fn shape_resources(shapes: &[Shape], owner: ShapeOwner, resources: &mut BTreeSet<SourceResource>) -> ContractResult<()> {
    for shape in shapes {
        if !resources.insert(SourceResource::Shape { owner, id: shape.id }) {
            return Err(ContractError::ResourceGraph);
        }
        shape_resources(shape.children(), owner, resources)?;
    }
    Ok(())
}

// Native model serialization has already passed finite-number, byte, node and depth limits.
// Discover references in every modeled fill/run/list style, not just top-level pictures/clicks.
fn discover_references(
    value: &Value,
    media: &BTreeSet<MediaId>,
    resources: &mut BTreeSet<SourceResource>,
    links: &mut BTreeSet<SlideId>,
) -> ContractResult<()> {
    match value {
        Value::Object(fields) => {
            if let Some(id) = fields.get("media") {
                let id = id.as_u64().and_then(|n| u32::try_from(n).ok()).map(MediaId).ok_or(ContractError::ResourceGraph)?;
                if !media.contains(&id) {
                    return Err(ContractError::ResourceGraph);
                }
                resources.insert(SourceResource::Media { id });
            }
            if let Some(action) = fields.get("action") {
                match action.get("kind").and_then(Value::as_str) {
                    Some("slide") => {
                        let id = action
                            .get("slide")
                            .and_then(Value::as_u64)
                            .and_then(|n| u32::try_from(n).ok())
                            .map(SlideId)
                            .ok_or(ContractError::ResourceGraph)?;
                        links.insert(id);
                    }
                    Some("url") => {
                        let url = action.get("url").and_then(Value::as_str).ok_or(ContractError::UnsupportedTemplate)?;
                        if !url.starts_with("https://")
                            || url.len() > 2048
                            || url.strip_prefix("https://").is_none_or(str::is_empty)
                            || url.contains('@')
                            || url.chars().any(|c| c.is_control() || c.is_whitespace())
                        {
                            return Err(ContractError::UnsupportedTemplate);
                        }
                    }
                    _ => return Err(ContractError::UnsupportedTemplate),
                }
            }
            for child in fields.values() {
                discover_references(child, media, resources, links)?;
            }
        }
        Value::Array(items) => {
            for child in items {
                discover_references(child, media, resources, links)?;
            }
        }
        _ => {}
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemplateSlot {
    pub key: String,
    pub shape_id: ShapeId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemplatePage {
    pub key: String,
    pub source_slide_id: SlideId,
    pub slots: Vec<TemplateSlot>,
}

/// Host-only authority. No deserializer can reconstruct it from a model response.
#[derive(Clone, Debug)]
pub struct AppendScope {
    snapshot: SnapshotIdentity,
    bundle: Arc<TemplateBundle>,
    pages: Vec<TemplatePage>,
    max_instances: usize,
}

impl AppendScope {
    pub fn freeze(
        snapshot: SnapshotIdentity,
        bundle: Arc<TemplateBundle>,
        pages: Vec<TemplatePage>,
        max_instances: usize,
        target: &Presentation,
    ) -> ContractResult<Self> {
        snapshot.validate_for_append()?;
        if snapshot.template_version != Some(bundle.version) {
            return Err(ContractError::SnapshotMismatch);
        }
        if max_instances == 0 || max_instances > MAX_TARGET_SLIDES || pages.is_empty() || pages.len() > MAX_TARGET_SLIDES {
            return Err(ContractError::Limit("append pages"));
        }
        let mut keys = BTreeSet::new();
        let mut slides = BTreeSet::new();
        let mut fields = 0usize;
        for page in &pages {
            validate_key(&page.key)?;
            if !keys.insert(&page.key) || !slides.insert(page.source_slide_id) {
                return Err(ContractError::DuplicateTarget);
            }
            let graph = bundle.graphs.get(&page.source_slide_id).ok_or(ContractError::UnauthorizedTarget)?;
            if graph.linked_slides.iter().any(|id| !pages.iter().any(|p| p.source_slide_id == *id)) {
                return Err(ContractError::UnauthorizedTarget);
            }
            let mut slot_keys = BTreeSet::new();
            let mut shapes = BTreeSet::new();
            for slot in &page.slots {
                fields = fields.checked_add(1).ok_or(ContractError::Limit("template slots"))?;
                if fields > 256 {
                    return Err(ContractError::Limit("template slots"));
                }
                validate_key(&slot.key)?;
                if !slot_keys.insert(&slot.key) || !shapes.insert(slot.shape_id) {
                    return Err(ContractError::DuplicateTarget);
                }
                safe_paragraph_count(&bundle.native, TextTarget { slide_id: page.source_slide_id, shape_id: slot.shape_id })?;
            }
        }
        let scope = Self { snapshot, bundle, pages, max_instances };
        scope.validate_target(target)?;
        Ok(scope)
    }

    pub fn snapshot(&self) -> &SnapshotIdentity {
        &self.snapshot
    }
    pub fn bundle(&self) -> &TemplateBundle {
        &self.bundle
    }
    pub fn pages(&self) -> &[TemplatePage] {
        &self.pages
    }
    pub fn max_instances(&self) -> usize {
        self.max_instances
    }

    pub fn linked_pages(&self, page_key: &str) -> ContractResult<Vec<&TemplatePage>> {
        let page = self.pages.iter().find(|p| p.key == page_key).ok_or(ContractError::UnauthorizedTarget)?;
        let graph = self.bundle.graphs.get(&page.source_slide_id).ok_or(ContractError::ResourceGraph)?;
        graph.linked_slides.iter().map(|id| self.pages.iter().find(|p| p.source_slide_id == *id).ok_or(ContractError::ResourceGraph)).collect()
    }

    /// P1 must additionally compare actual document UID/content revision before candidate work.
    pub fn validate_target(&self, target: &Presentation) -> ContractResult<()> {
        if !target.slides.iter().map(|s| s.id).eq(self.snapshot.target_slide_ids.iter().copied()) {
            return Err(ContractError::SnapshotMismatch);
        }
        if !target.slides.is_empty()
            && (target.slide_size != self.bundle.native.slide_size
                || target.default_text_style != self.bundle.native.default_text_style
                || target.header_footer != self.bundle.native.header_footer)
        {
            return Err(ContractError::UnsupportedTemplate);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlotReplacement {
    pub slot_key: String,
    pub paragraphs: Vec<String>,
}

/// Explicitly choose a generated instance of each linked source page. Repeated copies may link
/// to different copies; there is no first-instance/default/old-document fallback.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageLink {
    pub target_page_key: String,
    pub target_local_id: LocalId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageInstance {
    pub page_key: String,
    pub local_id: LocalId,
    pub replacements: Vec<SlotReplacement>,
    pub links: Vec<PageLink>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum AppendResponse {
    AppendTemplate { snapshot: SnapshotIdentity, pages: Vec<PageInstance> },
}

impl AppendResponse {
    pub fn decode(bytes: &[u8], scope: &AppendScope) -> ContractResult<Self> {
        let response: Self = decode_bounded(bytes)?;
        response.validate(scope)?;
        Ok(response)
    }

    pub fn pages(&self) -> &[PageInstance] {
        let Self::AppendTemplate { pages, .. } = self;
        pages
    }

    pub fn validate(&self, scope: &AppendScope) -> ContractResult<()> {
        let Self::AppendTemplate { snapshot, pages } = self;
        snapshot.validate_for_append()?;
        if snapshot != &scope.snapshot {
            return Err(ContractError::SnapshotMismatch);
        }
        if pages.is_empty() || pages.len() > scope.max_instances {
            return Err(ContractError::Limit("append instances"));
        }
        let mut local_ids = BTreeSet::new();
        for page in pages {
            if page.local_id.0 == 0 || !local_ids.insert(page.local_id) {
                return Err(ContractError::ResourceGraph);
            }
        }
        let mut media_bytes = 0usize;
        let mut bytes = 0usize;
        let mut operations = 0usize;
        for instance in pages {
            let page = scope.pages.iter().find(|p| p.key == instance.page_key).ok_or(ContractError::UnauthorizedTarget)?;
            let mut fields = BTreeSet::new();
            if instance.replacements.len() > page.slots.len() {
                return Err(ContractError::UnauthorizedTarget);
            }
            for replacement in &instance.replacements {
                operations = operations.checked_add(1).ok_or(ContractError::Limit("slot operations"))?;
                if operations > 256 {
                    return Err(ContractError::Limit("slot operations"));
                }
                let slot = page.slots.iter().find(|s| s.key == replacement.slot_key).ok_or(ContractError::UnauthorizedTarget)?;
                if !fields.insert(&replacement.slot_key) {
                    return Err(ContractError::DuplicateTarget);
                }
                if replacement.paragraphs.len() > MAX_PARAGRAPHS
                    || replacement.paragraphs.len()
                        != safe_paragraph_count(&scope.bundle.native, TextTarget { slide_id: page.source_slide_id, shape_id: slot.shape_id })?
                {
                    return Err(ContractError::ParagraphStructure);
                }
                let mut chars = 0usize;
                for text in &replacement.paragraphs {
                    bytes = bytes.checked_add(text.len()).ok_or(ContractError::Limit("draft text bytes"))?;
                    chars = chars.checked_add(text.chars().count()).ok_or(ContractError::Limit("draft text characters"))?;
                    if bytes > MAX_TOTAL_TEXT_BYTES || chars > MAX_TEXT_CHARS {
                        return Err(ContractError::Limit("draft text"));
                    }
                    if text.chars().any(char::is_control) {
                        return Err(ContractError::ControlCharacter);
                    }
                }
            }
            let graph = scope.bundle.graphs.get(&page.source_slide_id).ok_or(ContractError::ResourceGraph)?;
            for resource in &graph.resources {
                if let SourceResource::Media { id } = resource {
                    let media = scope.bundle.native.media(*id).ok_or(ContractError::ResourceGraph)?;
                    media_bytes = media_bytes.checked_add(media.data.len()).ok_or(ContractError::Limit("draft media bytes"))?;
                    if media_bytes > MAX_TOTAL_MEDIA_BYTES {
                        return Err(ContractError::Limit("draft media bytes"));
                    }
                }
            }
            if instance.links.len() != graph.linked_slides.len() {
                return Err(ContractError::ResourceGraph);
            }
            let mut linked = BTreeSet::new();
            for link in &instance.links {
                let source = scope.pages.iter().find(|p| p.key == link.target_page_key).ok_or(ContractError::UnauthorizedTarget)?;
                if !graph.linked_slides.contains(&source.source_slide_id) || !linked.insert(source.source_slide_id) {
                    return Err(ContractError::ResourceGraph);
                }
                if !pages.iter().any(|p| p.local_id == link.target_local_id && p.page_key == link.target_page_key) {
                    return Err(ContractError::ResourceGraph);
                }
            }
        }
        Ok(())
    }
}

/// Complete host draft: the actual resource package is retained, not just a lookup key. All
/// resource local IDs are assigned deterministically before P1 performs fresh destination remap.
#[derive(Clone, Debug)]
pub struct AppendDraft {
    scope: AppendScope,
    response: AppendResponse,
    mappings: Vec<ResourceMapping>,
}

impl AppendDraft {
    pub fn bind(scope: AppendScope, response: AppendResponse) -> ContractResult<Self> {
        response.validate(&scope)?;
        let mut next = response.pages().iter().map(|p| p.local_id.0).max().ok_or(ContractError::ResourceGraph)?;
        let mut mappings = Vec::new();
        for instance in response.pages() {
            let page = scope.pages.iter().find(|p| p.key == instance.page_key).ok_or(ContractError::UnauthorizedTarget)?;
            let graph = scope.bundle.graphs.get(&page.source_slide_id).ok_or(ContractError::ResourceGraph)?;
            for source in &graph.resources {
                if mappings.len() >= MAX_DRAFT_MAPPINGS {
                    return Err(ContractError::Limit("draft mappings"));
                }
                let local = if *source == (SourceResource::Slide { id: page.source_slide_id }) {
                    instance.local_id
                } else {
                    next = next.checked_add(1).ok_or(ContractError::Limit("local IDs"))?;
                    LocalId(next)
                };
                mappings.push(ResourceMapping { instance: instance.local_id, source: *source, local });
            }
        }
        let draft = Self { scope, response, mappings };
        draft.validate()?;
        Ok(draft)
    }

    pub fn scope(&self) -> &AppendScope {
        &self.scope
    }
    pub fn response(&self) -> &AppendResponse {
        &self.response
    }
    pub fn mappings(&self) -> &[ResourceMapping] {
        &self.mappings
    }

    /// Recheck complete dependency closure at the engine boundary even for an in-memory draft.
    /// Installation must fresh-map every local resource, rewrite links via explicit PageLink,
    /// and prepare all native copies/media before changing the live document or history.
    pub fn validate(&self) -> ContractResult<()> {
        self.response.validate(&self.scope)?;
        if self.mappings.len() > MAX_DRAFT_MAPPINGS {
            return Err(ContractError::Limit("draft mappings"));
        }
        let mut locals = BTreeSet::new();
        let mut actual = BTreeSet::new();
        for mapping in &self.mappings {
            if mapping.local.0 == 0 || !locals.insert(mapping.local) || !actual.insert((mapping.instance, mapping.source)) {
                return Err(ContractError::ResourceGraph);
            }
        }
        let mut expected = BTreeSet::new();
        for instance in self.response.pages() {
            let page = self.scope.pages.iter().find(|p| p.key == instance.page_key).ok_or(ContractError::UnauthorizedTarget)?;
            let graph = self.scope.bundle.graphs.get(&page.source_slide_id).ok_or(ContractError::ResourceGraph)?;
            for source in &graph.resources {
                expected.insert((instance.local_id, *source));
                let mapping =
                    self.mappings.iter().find(|m| m.instance == instance.local_id && m.source == *source).ok_or(ContractError::ResourceGraph)?;
                if *source == (SourceResource::Slide { id: page.source_slide_id }) && mapping.local != instance.local_id {
                    return Err(ContractError::ResourceGraph);
                }
            }
        }
        if actual != expected {
            return Err(ContractError::ResourceGraph);
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "ai_contract_template_tests.rs"]
mod tests;
