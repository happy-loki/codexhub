use anyhow::Result;

use crate::{
    app_state::SharedState,
    im::core::{
        i18n::im_text_for_state,
        thread::{
            ThreadListEntry, build_thread_entries, load_codex_app_model_provider,
            next_thread_routing_request_id,
        },
    },
    im_runtime::{
        RouteTarget, ThreadCreateDraftState, ThreadRoutingRequestState, ThreadRoutingStage,
    },
    remote_control_backend,
};

const THREAD_HISTORY_PAGE_SIZE: u32 = 8;
const THREAD_LOADED_LIMIT: u32 = 64;

#[derive(Debug, Clone)]
pub(crate) struct ThreadRoutingPage {
    pub(crate) request_id: String,
    pub(crate) page: usize,
    pub(crate) page_cursors: Vec<Option<String>>,
    pub(crate) thread_ids_by_page: Vec<Vec<String>>,
    pub(crate) entries: Vec<ThreadListEntry>,
    pub(crate) next_cursor: Option<String>,
    pub(crate) model_provider_filter: Option<String>,
}

impl ThreadRoutingPage {
    pub(crate) fn into_request(
        self,
        route: &RouteTarget,
        message_id: String,
        existing_request: Option<&ThreadRoutingRequestState>,
        cursor: Option<&str>,
    ) -> ThreadRoutingRequestState {
        ThreadRoutingRequestState {
            request_id: self.request_id,
            conversation_key: route.conversation_key.clone(),
            account_id: route.account_id.clone(),
            chat_id: route.chat_id.clone(),
            message_id: Some(message_id),
            stage: ThreadRoutingStage::ResumeList,
            page: self.page,
            page_cursors: self.page_cursors,
            thread_ids_by_page: self.thread_ids_by_page,
            create_draft: existing_request
                .map(|request| request.create_draft.clone())
                .unwrap_or_default(),
            create_option_values_by_field_page: existing_request
                .map(|request| request.create_option_values_by_field_page.clone())
                .unwrap_or_default(),
            history_cursor: cursor.map(str::to_string),
            history_has_next: self.next_cursor.is_some(),
        }
    }
}

pub(crate) async fn load_thread_routing_page(
    state: &SharedState,
    route: &RouteTarget,
    existing_request: Option<&ThreadRoutingRequestState>,
    cursor: Option<&str>,
    page: usize,
) -> Result<ThreadRoutingPage> {
    let request_id = existing_request
        .map(|request| request.request_id.clone())
        .unwrap_or_else(next_thread_routing_request_id);
    let mut page_cursors = existing_request
        .map(|request| request.page_cursors.clone())
        .unwrap_or_else(|| vec![None]);
    if page_cursors.len() < page {
        page_cursors.resize(page, None);
    }
    page_cursors[page - 1] = cursor.map(str::to_string);

    let client_key = route.remote_client_key.clone();
    let loaded_ids = match remote_control_backend::thread_loaded_list_for_client(
        state,
        &client_key,
        None,
        Some(THREAD_LOADED_LIMIT),
    )
    .await
    {
        Ok(loaded) => loaded
            .get("data")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect::<Vec<_>>(),
        Err(err) => {
            state
                .push_event("warn", "thread_loaded_list_failed", err.to_string())
                .await;
            Vec::new()
        }
    };
    let model_provider_filter = load_codex_app_model_provider();
    let history = remote_control_backend::thread_list_for_client(
        state,
        &client_key,
        cursor,
        Some(THREAD_HISTORY_PAGE_SIZE),
        None,
        model_provider_filter.as_deref(),
    )
    .await?;
    let history_threads = history
        .get("data")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let current_thread_id =
        remote_control_backend::current_thread_for_client(state, &client_key).await;
    let text = im_text_for_state(state);
    let entries = build_thread_entries(
        &loaded_ids,
        &history_threads,
        current_thread_id.as_deref(),
        text,
    );
    let thread_ids = entries
        .iter()
        .map(|entry| entry.thread_id.clone())
        .collect::<Vec<_>>();
    let mut thread_ids_by_page = existing_request
        .map(|request| request.thread_ids_by_page.clone())
        .unwrap_or_else(|| vec![Vec::new()]);
    if thread_ids_by_page.len() < page {
        thread_ids_by_page.resize(page, Vec::new());
    }
    thread_ids_by_page[page - 1] = thread_ids;
    let next_cursor = history
        .get("nextCursor")
        .or_else(|| history.get("next_cursor"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    if page_cursors.len() <= page {
        page_cursors.resize(page + 1, None);
    }
    page_cursors[page] = next_cursor.clone();

    Ok(ThreadRoutingPage {
        request_id,
        page,
        page_cursors,
        thread_ids_by_page,
        entries,
        next_cursor,
        model_provider_filter,
    })
}

pub(crate) fn empty_thread_routing_request(
    route: &RouteTarget,
    request_id: String,
    message_id: String,
) -> ThreadRoutingRequestState {
    ThreadRoutingRequestState {
        request_id,
        conversation_key: route.conversation_key.clone(),
        account_id: route.account_id.clone(),
        chat_id: route.chat_id.clone(),
        message_id: Some(message_id),
        stage: ThreadRoutingStage::Choice,
        page: 1,
        page_cursors: vec![None],
        thread_ids_by_page: vec![vec![]],
        create_draft: ThreadCreateDraftState::default(),
        create_option_values_by_field_page: Default::default(),
        history_cursor: None,
        history_has_next: false,
    }
}
