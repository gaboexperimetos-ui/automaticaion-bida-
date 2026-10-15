//! Quiz data structures.

use serde::{Deserialize, Serialize};

/// A generated quiz with title and questions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Quiz {
    pub title: String,
    pub questions: Vec<QuizQuestion>,
}

/// A single quiz question.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuizQuestion {
    pub question: String,
    pub options: Vec<String>,
    /// Index of the correct answer in `options`
    pub correct_answer: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quiz_creation() {
        let quiz = Quiz {
            title: "Math Quiz".to_string(),
            questions: vec![QuizQuestion {
                question: "2+2?".to_string(),
                options: vec!["3".into(), "4".into(), "5".into()],
                correct_answer: 1,
            }],
        };
        assert_eq!(quiz.title, "Math Quiz");
        assert_eq!(quiz.questions[0].correct_answer, 1);
    }

    #[test]
    fn test_quiz_json_roundtrip() {
        let quiz = Quiz {
            title: "Test".to_string(),
            questions: vec![QuizQuestion {
                question: "Q?".to_string(),
                options: vec!["a".into(), "b".into(), "c".into(), "d".into()],
                correct_answer: 2,
            }],
        };
        let json = serde_json::to_string(&quiz).unwrap();
        let parsed: Quiz = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.title, "Test");
    }
}
