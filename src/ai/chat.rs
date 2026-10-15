//! Chat service with persistence and subject context.

use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::ai::openrouter::{
    chat_completion, persistent_summary, AiError, ChatMessage, CostTracker,
};
use crate::config::Config;

/// A conversation record persisted in the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: i64,
    pub subject: String,
    pub role: String,
    pub content: String,
    pub created_at: String,
}

/// AI assistant providing chat, summarization, quiz, and study sheet generation.
/// Uses OpenRouter when API key is configured; falls back to mock mode otherwise.
pub struct AiAssistant {
    state: Arc<crate::AppState>,
    cost_tracker: Arc<tokio::sync::Mutex<CostTracker>>,
}

impl AiAssistant {
    pub fn new(state: Arc<crate::AppState>) -> Self {
        let budget = state.config.openrouter_max_cost_usd;
        Self {
            state,
            cost_tracker: Arc::new(tokio::sync::Mutex::new(CostTracker::new(budget))),
        }
    }

    /// Get the cost summary (async — requires tokio runtime).
    ///
    /// Persistent: reads the current month's spend from the `usage_metrics`
    /// table, so it survives restarts and `AiAssistant` re-creation
    /// (handlers build a new instance per request). Falls back to the
    /// in-memory tracker only if the DB query fails.
    pub async fn cost_tracking(&self) -> crate::ai::CostSummary {
        match persistent_summary(&self.state.db, &self.state.config).await {
            Ok(summary) => summary,
            Err(e) => {
                tracing::warn!("ai cost query failed, using in-memory tracker");
                let _ = e.log_detail();
                self.cost_tracker.lock().await.summary()
            }
        }
    }

    /// Get remaining budget (async — requires tokio runtime).
    ///
    /// Persistent (DB-backed); falls back to the in-memory tracker on DB error.
    pub async fn budget_remaining(&self) -> f64 {
        let budget = self.state.config.openrouter_max_cost_usd;
        match crate::ai::openrouter::month_spend(&self.state.db).await {
            Ok(spent) => budget - spent,
            Err(_) => {
                let t = self.cost_tracker.lock().await;
                t.budget_limit - t.total_cost
            }
        }
    }

    /// Send a chat message and get a response.
    pub async fn chat(&self, message: &str, subject: &str) -> Result<String, AiError> {
        let config = &self.state.config;
        let max_tokens = config.openrouter_max_tokens;

        let messages = vec![
            ChatMessage {
                role: "system".to_string(),
                content: format!("Eres un asistente académico especializado en {}. Responde en español de forma concisa y útil.", subject),
            },
            ChatMessage {
                role: "user".to_string(),
                content: message.to_string(),
            },
        ];

        // Track conversation in DB
        self.save_conversation("assistant", "system", &messages[0].content, subject)
            .await
            .ok();
        self.save_conversation("assistant", "user", message, subject)
            .await
            .ok();

        let mut t = self.cost_tracker.lock().await;
        let response = chat_completion(
            &self.state.db,
            config,
            &mut *t,
            "chat",
            messages,
            max_tokens,
        )
        .await?;

        // Save assistant response
        self.save_conversation("assistant", "assistant", &response, subject)
            .await
            .ok();
        Ok(response)
    }

    /// Generate a summary from text content.
    pub async fn summarize(&self, content: &str, max_tokens: u32) -> Result<String, AiError> {
        let config = &self.state.config;
        let prompt = format!(
            "Resume el siguiente texto académico en español, máximo {} tokens. Extrae los puntos clave, definiciones y fórmulas importantes:\n\n{}",
            max_tokens.min(500), &content[..content.len().min(4000)]
        );

        let messages = vec![ChatMessage {
            role: "user".to_string(),
            content: prompt,
        }];

        let tracker = self.cost_tracker.clone();
        let mut t = tracker.lock().await;
        let response = chat_completion(
            &self.state.db,
            config,
            &mut *t,
            "summary",
            messages,
            max_tokens.min(500),
        )
        .await?;
        Ok(response)
    }

    /// Generate a quiz from content.
    pub async fn generate_quiz(
        &self,
        content: &str,
        num_questions: usize,
    ) -> Result<crate::ai::Quiz, AiError> {
        let config = &self.state.config;
        let n = num_questions.clamp(1, 10);
        let prompt = format!(
            "Genera un cuestionario de {} preguntas de opción múltiple (4 opciones cada una) a partir del siguiente texto. Devuelve en formato JSON con estructura: {{\"title\": \"...\", \"questions\": [{{\"question\": \"...\", \"options\": [\"a\", \"b\", \"c\", \"d\"], \"correct_answer\": 0}}]}}\n\n{}",
            n, &content[..content.len().min(4000)]
        );

        let messages = vec![ChatMessage {
            role: "user".to_string(),
            content: prompt,
        }];

        let tracker = self.cost_tracker.clone();
        let mut t = tracker.lock().await;
        let response =
            chat_completion(&self.state.db, config, &mut *t, "quiz", messages, 2000).await?;

        // Parse JSON or use mock
        match serde_json::from_str::<crate::ai::Quiz>(&response) {
            Ok(quiz) => Ok(quiz),
            Err(_) => {
                // Generate mock quiz
                Ok(crate::ai::Quiz {
                    title: "Cuestionario".to_string(),
                    questions: (0..n)
                        .map(|i| crate::ai::QuizQuestion {
                            question: format!(
                                "Pregunta {}: ¿Cuál es el concepto clave del tema {}?",
                                i + 1,
                                i + 1
                            ),
                            options: vec![
                                "Opción A".to_string(),
                                "Opción B".to_string(),
                                "Opción C".to_string(),
                                "Opción D".to_string(),
                            ],
                            correct_answer: 0,
                        })
                        .collect(),
                })
            }
        }
    }

    /// Generate a study sheet from content.
    pub async fn generate_study_sheet(&self, content: &str) -> Result<String, AiError> {
        let config = &self.state.config;
        let prompt = format!(
            "Genera una hoja de estudio en Markdown con título, resumen, fórmulas clave, y ejemplos. Máximo 800 tokens:\n\n{}",
            &content[..content.len().min(4000)]
        );

        let messages = vec![ChatMessage {
            role: "user".to_string(),
            content: prompt,
        }];

        let tracker = self.cost_tracker.clone();
        let mut t = tracker.lock().await;
        let response = chat_completion(
            &self.state.db,
            config,
            &mut *t,
            "study-sheet",
            messages,
            800,
        )
        .await?;
        Ok(response)
    }

    async fn save_conversation(
        &self,
        _assistant: &str,
        role: &str,
        content: &str,
        subject: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("INSERT INTO chat_history (subject, role, content) VALUES (?, ?, ?)")
            .bind(subject)
            .bind(role)
            .bind(content)
            .execute(&self.state.db)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quiz_serialization() {
        let q = crate::ai::QuizQuestion {
            question: "Test?".to_string(),
            options: vec![
                "A".to_string(),
                "B".to_string(),
                "C".to_string(),
                "D".to_string(),
            ],
            correct_answer: 2,
        };
        let json = serde_json::to_string(&q).unwrap();
        let parsed: crate::ai::QuizQuestion = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.correct_answer, 2);
    }
}
