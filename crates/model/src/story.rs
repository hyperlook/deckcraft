//! The deck's agreed story and the background materials for one piece of work.
//!
//! Hand edits on a slide do not update this. A slide changes from here only when an AI proposal
//! for that page is accepted.

use serde::{Deserialize, Serialize};

use crate::SlideId;

/// One page in the agreed story.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct OutlinePage {
    /// Deck slide this entry describes. Empty when the page is planned and has no slide yet.
    pub page_id: Option<SlideId>,
    pub title: String,
    pub points: Vec<String>,
}

/// The agreed story for the deck. This is a source of truth, not a cache of whatever text is
/// currently in the text boxes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Outline {
    pub pages: Vec<OutlinePage>,
}

impl Outline {
    pub fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }

    /// Replace the entry for `page`, or append one. Other pages stay as they are.
    pub fn upsert(&mut self, page: SlideId, title: String, points: Vec<String>) {
        if let Some(entry) = self.pages.iter_mut().find(|entry| entry.page_id == Some(page)) {
            entry.title = title;
            entry.points = points;
            return;
        }
        self.pages.push(OutlinePage { page_id: Some(page), title, points });
    }
}

/// What kind of background material this is. The bytes of an image stay with the host; only the
/// name and the text already taken from it are kept here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MaterialKind {
    #[default]
    Text,
    Image,
    File,
}

/// One piece of background for this deck: a note, a file's extracted text, or an image caption.
/// This is not a filesystem path and not permission to read anything else.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Material {
    pub name: String,
    pub kind: MaterialKind,
    pub body: String,
}
