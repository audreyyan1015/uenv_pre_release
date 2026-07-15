// 文件职责：实现内部 EpisodeService trait，将批量提交和单次提交转接到 UEnvEpisodeService。
// 主要功能：为 adapter-core 提供统一 episode service 接口，规范化请求并并发执行 batch。
// 大致工作流：调用方通过 trait 提交请求；本文件 clone state、构造临时 service，并调用 submit_episode/submit_episode_batch。

impl EpisodeService for UEnvEpisodeService {
    async fn submit_episode_batch(
        &self,
        requests: Vec<EpisodeRequest>,
    ) -> Result<Vec<EpisodeResult>, EpisodeServiceError> {
        let state = Arc::clone(&self.state);
        let futures = requests.into_iter().map(|mut req| {
            normalize_episode_request(&mut req);
            let episode_id = req.episode_id.clone();
            let state = Arc::clone(&state);
            async move {
                match (UEnvEpisodeService { state }).submit_episode(req).await {
                    Ok(result) => result,
                    Err(e) => {
                        let mut failed_req = EpisodeRequest {
                            episode_id,
                            ..Default::default()
                        };
                        let _ = ensure_async_request_context(&mut failed_req);
                        failed_result_from_request(
                            &failed_req,
                            "failed",
                            e.to_string(),
                            ErrorCode::ErrInternal,
                            None,
                        )
                    }
                }
            }
        });
        Ok(join_all(futures).await)
    }
}
