const DEFAULT_WINDOW_FRAMES: usize = 2_048;
const MIN_WINDOW_FRAMES: usize = 64;
const MAX_SEARCH_RADIUS_FRAMES: usize = 512;
const CORRELATION_STRIDE: usize = 16;
const CANDIDATE_STRIDE: usize = 8;

/// Changes stereo duration while preserving local waveform pitch with deterministic WSOLA.
///
/// Smart DJ limits tempo changes to ±6%, where short-window waveform matching produces
/// stable results without native libraries or generated bindings.
pub(crate) fn pitch_preserving_stretch(
    input: &[[f32; 2]],
    speed_ratio: f32,
) -> Vec<[f32; 2]> {
    if input.is_empty() {
        return Vec::new();
    }
    if !speed_ratio.is_finite() || speed_ratio <= 0.0 {
        return input.to_vec();
    }
    if (speed_ratio - 1.0).abs() < 0.0005 {
        return input.to_vec();
    }

    let target_frames = ((input.len() as f64 / speed_ratio as f64).round() as usize).max(1);
    if input.len() < MIN_WINDOW_FRAMES || target_frames < MIN_WINDOW_FRAMES {
        return resample_linear(input, target_frames);
    }

    let window_frames = DEFAULT_WINDOW_FRAMES
        .min(input.len())
        .min(target_frames)
        .max(MIN_WINDOW_FRAMES);
    let overlap_frames = (window_frames / 2).max(1);
    let synthesis_hop = window_frames - overlap_frames;
    let analysis_hop = synthesis_hop as f64 * speed_ratio as f64;
    let search_radius = (window_frames / 4).min(MAX_SEARCH_RADIUS_FRAMES);
    let max_input_start = input.len().saturating_sub(window_frames);

    let mut output = vec![[0.0; 2]; target_frames];
    let initial_frames = window_frames.min(target_frames).min(input.len());
    output[..initial_frames].copy_from_slice(&input[..initial_frames]);

    let mut synthesis_position = synthesis_hop;
    let mut previous_input_position = 0usize;

    while synthesis_position < target_frames {
        let expected_input_position = (previous_input_position as f64 + analysis_hop)
            .round()
            .clamp(0.0, max_input_start as f64) as usize;
        let search_start = expected_input_position.saturating_sub(search_radius);
        let search_end = expected_input_position
            .saturating_add(search_radius)
            .min(max_input_start);
        let overlap_length = overlap_frames
            .min(target_frames - synthesis_position)
            .min(window_frames);

        let best_input_position = best_matching_position(
            &output,
            synthesis_position,
            input,
            search_start,
            search_end,
            expected_input_position,
            overlap_length,
        );
        let segment_length = window_frames.min(target_frames - synthesis_position);
        let blend_length = overlap_length.min(segment_length);

        for offset in 0..blend_length {
            let phase = (offset + 1) as f32 / (blend_length + 1) as f32;
            let fade_in = 0.5 - 0.5 * (std::f32::consts::PI * phase).cos();
            let fade_out = 1.0 - fade_in;
            let existing = output[synthesis_position + offset];
            let incoming = input[best_input_position + offset];
            output[synthesis_position + offset] = [
                existing[0] * fade_out + incoming[0] * fade_in,
                existing[1] * fade_out + incoming[1] * fade_in,
            ];
        }

        if segment_length > blend_length {
            let output_start = synthesis_position + blend_length;
            let output_end = synthesis_position + segment_length;
            let input_start = best_input_position + blend_length;
            let input_end = best_input_position + segment_length;
            output[output_start..output_end].copy_from_slice(&input[input_start..input_end]);
        }

        previous_input_position = best_input_position;
        synthesis_position = synthesis_position.saturating_add(synthesis_hop);
    }

    output
}

fn best_matching_position(
    output: &[[f32; 2]],
    output_position: usize,
    input: &[[f32; 2]],
    search_start: usize,
    search_end: usize,
    expected_position: usize,
    overlap_length: usize,
) -> usize {
    if search_start >= search_end || overlap_length == 0 {
        return search_start;
    }

    let mut best_position = expected_position.clamp(search_start, search_end);
    let mut best_score = f64::NEG_INFINITY;
    let mut best_distance = usize::MAX;

    let mut candidate = search_start;
    loop {
        let score = similarity_score(
            output,
            output_position,
            input,
            candidate,
            overlap_length,
        );
        let distance = candidate.abs_diff(expected_position);
        if score > best_score + f64::EPSILON
            || ((score - best_score).abs() <= f64::EPSILON && distance < best_distance)
        {
            best_score = score;
            best_position = candidate;
            best_distance = distance;
        }

        if candidate >= search_end {
            break;
        }
        candidate = candidate
            .saturating_add(CANDIDATE_STRIDE)
            .min(search_end);
    }

    let refine_start = best_position.saturating_sub(CANDIDATE_STRIDE);
    let refine_end = best_position
        .saturating_add(CANDIDATE_STRIDE)
        .min(search_end);
    for candidate in refine_start..=refine_end {
        if candidate < search_start {
            continue;
        }
        let score = similarity_score(
            output,
            output_position,
            input,
            candidate,
            overlap_length,
        );
        let distance = candidate.abs_diff(expected_position);
        if score > best_score + f64::EPSILON
            || ((score - best_score).abs() <= f64::EPSILON && distance < best_distance)
        {
            best_score = score;
            best_position = candidate;
            best_distance = distance;
        }
    }

    best_position
}

fn similarity_score(
    output: &[[f32; 2]],
    output_position: usize,
    input: &[[f32; 2]],
    input_position: usize,
    overlap_length: usize,
) -> f64 {
    let mut dot = 0.0f64;
    let mut output_energy = 0.0f64;
    let mut input_energy = 0.0f64;

    for offset in (0..overlap_length).step_by(CORRELATION_STRIDE) {
        let existing = output[output_position + offset];
        let candidate = input[input_position + offset];
        for channel in 0..2 {
            let left = existing[channel] as f64;
            let right = candidate[channel] as f64;
            dot += left * right;
            output_energy += left * left;
            input_energy += right * right;
        }
    }

    const SILENCE_ENERGY: f64 = 1.0e-12;
    if output_energy <= SILENCE_ENERGY && input_energy <= SILENCE_ENERGY {
        return 0.0;
    }
    if output_energy <= SILENCE_ENERGY || input_energy <= SILENCE_ENERGY {
        return -1.0;
    }

    dot / (output_energy * input_energy).sqrt()
}

fn resample_linear(input: &[[f32; 2]], target_frames: usize) -> Vec<[f32; 2]> {
    if target_frames == 0 || input.is_empty() {
        return Vec::new();
    }
    if target_frames == 1 || input.len() == 1 {
        return vec![input[0]; target_frames];
    }

    let scale = (input.len() - 1) as f64 / (target_frames - 1) as f64;
    (0..target_frames)
        .map(|index| {
            let source_position = index as f64 * scale;
            let left_index = source_position.floor() as usize;
            let right_index = (left_index + 1).min(input.len() - 1);
            let fraction = (source_position - left_index as f64) as f32;
            [
                input[left_index][0] * (1.0 - fraction) + input[right_index][0] * fraction,
                input[left_index][1] * (1.0 - fraction) + input[right_index][1] * fraction,
            ]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::pitch_preserving_stretch;

    #[test]
    fn keeps_unity_ratio_unchanged() {
        let input = vec![[0.25, -0.25]; 512];
        assert_eq!(pitch_preserving_stretch(&input, 1.0), input);
    }

    #[test]
    fn produces_expected_duration() {
        let input = (0..44_100)
            .map(|index| {
                let phase = index as f32 * 0.01;
                [phase.sin() * 0.5, phase.cos() * 0.5]
            })
            .collect::<Vec<_>>();
        let output = pitch_preserving_stretch(&input, 1.05);
        assert_eq!(output.len(), (input.len() as f64 / 1.05).round() as usize);
        assert!(output.iter().flatten().all(|sample| sample.is_finite()));
    }

    #[test]
    fn preserves_constant_stereo_signal() {
        let input = vec![[0.25, -0.5]; 8_192];
        let output = pitch_preserving_stretch(&input, 0.95);
        assert!(output
            .iter()
            .all(|frame| (frame[0] - 0.25).abs() < 1.0e-6 && (frame[1] + 0.5).abs() < 1.0e-6));
    }
}
