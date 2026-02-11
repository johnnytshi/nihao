use crate::embed::Embedding;

/// Match result containing the best matching face
#[derive(Debug, Clone)]
pub struct MatchResult {
    pub face_id: usize,
    pub similarity: f32,
}

/// Compute cosine similarity between two L2-normalized embeddings
/// For normalized vectors, this is simply the dot product
pub fn cosine_similarity(a: &Embedding, b: &Embedding) -> f32 {
    a.dot(b)
}

/// Find the best match above threshold
pub fn find_best_match(
    query: &Embedding,
    candidates: &[Embedding],
    threshold: f32,
) -> Option<MatchResult> {
    candidates
        .iter()
        .enumerate()
        .map(|(idx, candidate)| (idx, cosine_similarity(query, candidate)))
        .filter(|(_, sim)| *sim >= threshold)
        .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(face_id, similarity)| MatchResult { face_id, similarity })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ndarray::arr1;

    #[test]
    fn test_cosine_similarity() {
        let a = arr1(&[1.0, 0.0, 0.0]);
        let b = arr1(&[1.0, 0.0, 0.0]);
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 1e-6);

        let c = arr1(&[0.0, 1.0, 0.0]);
        assert!(cosine_similarity(&a, &c).abs() < 1e-6);
    }

    #[test]
    fn test_find_best_match() {
        let query = arr1(&[1.0, 0.0, 0.0]);
        let candidates = vec![
            arr1(&[0.9, 0.1, 0.0]),
            arr1(&[0.8, 0.2, 0.0]),
            arr1(&[0.0, 1.0, 0.0]),
        ];

        let result = find_best_match(&query, &candidates, 0.5);
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.face_id, 0);
        assert!(result.similarity > 0.8);

        let no_match = find_best_match(&query, &candidates, 0.95);
        assert!(no_match.is_none());
    }
}
