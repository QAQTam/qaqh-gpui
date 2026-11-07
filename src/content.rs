//! Resolving a reference never fabricates content or a successful read.
use crate::store::SessionStore;
use qaqh_client::ContentRef;
use qaqh_ringing_contract::{fact::ContentValue, state::TimelineEntry};

fn values_mut(store: &mut SessionStore, mut visit: impl FnMut(&mut ContentValue)) {
    for entry in &mut store.entries {
        match entry {
            TimelineEntry::Input { content, .. }
            | TimelineEntry::AssistantBlock { content, .. } => visit(content),
            _ => {}
        }
    }
    for tool in &mut store.tools {
        if let Some(value) = &mut tool.output {
            visit(value);
        }
    }
    for interaction in &mut store.interactions {
        visit(&mut interaction.request);
    }
    for agent in &mut store.subagents {
        if let Some(value) = &mut agent.result {
            visit(value);
        }
    }
}
pub fn references(store: &mut SessionStore) -> Vec<ContentRef> {
    let mut references = Vec::new();
    values_mut(store, |value| {
        if let ContentValue::Ref { content_ref } = value
            && !references.contains(content_ref)
        {
            references.push(content_ref.clone());
        }
    });
    references
}
pub fn replace(store: &mut SessionStore, reference: &ContentRef, text: String) {
    values_mut(store, |value| {
        if let ContentValue::Ref { content_ref } = value
            && content_ref == reference
        {
            *value = ContentValue::Inline { text: text.clone() };
        }
    });
    store.timeline_list.reset(store.entries.len() + 1);
}
