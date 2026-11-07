use std::collections::HashSet;

use huskarl::token::AccessToken;
use serde::Deserialize;

use crate::{error::GraphError, graph_url::GraphUrl, transport::GraphOperation};

use super::GraphApi;

pub(super) const MAX_PAGINATION_PAGES: usize = 1_000;
pub(super) const MAX_PAGINATION_RESPONSE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GraphPage<T> {
    #[serde(rename = "value")]
    pub(super) records: Vec<T>,
    #[serde(rename = "@odata.nextLink")]
    pub(super) next_link: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GraphDeltaPage<T> {
    #[serde(rename = "value")]
    pub(super) records: Vec<T>,
    #[serde(rename = "@odata.nextLink")]
    pub(super) next_link: Option<String>,
    #[serde(rename = "@odata.deltaLink")]
    pub(super) delta_link: Option<String>,
}

pub(super) struct PageTraversal<'a> {
    api: &'a GraphApi,
    operation: GraphOperation,
    token: std::sync::Arc<AccessToken>,
    budget: PaginationBudget,
}

impl<'a> PageTraversal<'a> {
    pub(super) fn new(
        api: &'a GraphApi,
        operation: GraphOperation,
        token: std::sync::Arc<AccessToken>,
    ) -> Self {
        Self {
            api,
            operation,
            token,
            budget: PaginationBudget::default(),
        }
    }

    pub(super) async fn collect<T, U>(
        &mut self,
        start: GraphUrl,
        mut convert: impl FnMut(T) -> Result<U, GraphError>,
    ) -> Result<Vec<U>, GraphError>
    where
        T: for<'de> Deserialize<'de>,
    {
        let mut next = start;
        let mut seen = HashSet::new();
        let mut records = Vec::new();
        loop {
            if !seen.insert(next.as_str().to_owned()) {
                return Err(GraphError::UnsafeLink);
            }
            let page = self.load_page(next).await?;
            for record in page.records {
                records.push(convert(record)?);
            }
            match page.next_link {
                Some(link) => next = self.api.authority.opaque_link(&link)?,
                None => return Ok(records),
            }
        }
    }

    pub(super) async fn load_page<T>(&mut self, url: GraphUrl) -> Result<GraphPage<T>, GraphError>
    where
        T: for<'de> Deserialize<'de>,
    {
        self.budget.begin_page()?;
        let body = self
            .api
            .transport
            .get(self.operation, url, std::sync::Arc::clone(&self.token))
            .await?;
        self.budget.add_response_bytes(body.len())?;
        serde_json::from_slice(&body).map_err(|_| GraphError::InvalidResponse)
    }
}

#[derive(Default)]
pub(super) struct PaginationBudget {
    pages: usize,
    response_bytes: usize,
}

impl PaginationBudget {
    pub(super) fn begin_page(&mut self) -> Result<(), GraphError> {
        if self.pages >= MAX_PAGINATION_PAGES {
            return Err(GraphError::PaginationLimitExceeded);
        }
        self.pages += 1;
        Ok(())
    }

    pub(super) fn add_response_bytes(&mut self, bytes: usize) -> Result<(), GraphError> {
        let Some(total) = self.response_bytes.checked_add(bytes) else {
            return Err(GraphError::PaginationLimitExceeded);
        };
        if total > MAX_PAGINATION_RESPONSE_BYTES {
            return Err(GraphError::PaginationLimitExceeded);
        }
        self.response_bytes = total;
        Ok(())
    }
}
