import { createServerFn } from "@tanstack/react-start";
import { z } from "zod";

const emptyableInt = z.string().trim().regex(/^\d*$/);
const trainingSteps = z
  .string()
  .trim()
  .refine((value) => value === "" || value === "null" || /^\d+$/.test(value), {
    message: "训练步数必须是正整数，或填写 null。",
  });
const decimal = z.string().trim().regex(/^(?:\d+(?:\.\d*)?|\.\d+)?$/);
const runId = z.string().trim().regex(/^[A-Za-z0-9_.-]*$/);

const launchVerlSchema = z.object({
  option_id: z.literal("verl"),
  run_id: runId,
  model_path: z.string().trim(),
  dataset_path: z.string().trim(),
  rl_algorithm: z.literal("GRPO"),
  limit: emptyableInt,
  offset: emptyableInt,
  training_steps: trainingSteps,
  total_epochs: emptyableInt,
  train_batch_size: emptyableInt,
  ppo_mini_batch_size: emptyableInt,
  rollout_n: emptyableInt,
  temperature: decimal,
  episode_max_steps: emptyableInt,
  max_prompt_length: emptyableInt,
  max_response_length: emptyableInt,
  parallel_mode: z.enum(["sync", "fully_async"]),
  save_freq: emptyableInt,
});

const stopVerlSchema = z.object({
  run_id: z.string().trim().regex(/^[A-Za-z0-9_.-]+$/),
  pid: z.number().int().positive(),
});

export const launchVerlTraining = createServerFn({ method: "POST" })
  .inputValidator(launchVerlSchema)
  .handler(async ({ data }) => {
    const { launchVerlPreset } = await import("./launch-task.server");
    return launchVerlPreset(data);
  });

export const stopVerlTraining = createServerFn({ method: "POST" })
  .inputValidator(stopVerlSchema)
  .handler(async ({ data }) => {
    const { stopVerlPreset } = await import("./launch-task.server");
    return stopVerlPreset(data);
  });
