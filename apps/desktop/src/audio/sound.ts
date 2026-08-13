export const SOUND_CUES = [
  "agent-needs-you",
  "agent-finished",
  "repomind-needs-you",
  "error-or-stall",
  "incoming-message",
  "update-ready",
] as const;

export type SoundCue = typeof SOUND_CUES[number];

export interface SoundPlayer {
  /** Returns true when custom audio was scheduled successfully. */
  play(cue: SoundCue, volume: number): boolean;
}

interface Tone {
  frequency: number;
  offset: number;
  duration: number;
}

const CONTOURS: Record<SoundCue, Tone[]> = {
  "agent-needs-you": [
    { frequency: 440.00, offset: 0.00, duration: 0.18 },
    { frequency: 554.37, offset: 0.15, duration: 0.24 },
  ],
  "agent-finished": [
    { frequency: 659.25, offset: 0.00, duration: 0.18 },
    { frequency: 493.88, offset: 0.15, duration: 0.26 },
  ],
  "repomind-needs-you": [
    { frequency: 293.66, offset: 0.00, duration: 0.17 },
    { frequency: 440.00, offset: 0.14, duration: 0.17 },
    { frequency: 587.33, offset: 0.28, duration: 0.25 },
  ],
  "error-or-stall": [
    { frequency: 220.00, offset: 0.00, duration: 0.20 },
    { frequency: 155.56, offset: 0.17, duration: 0.30 },
  ],
  "incoming-message": [
    { frequency: 523.25, offset: 0.00, duration: 0.16 },
    { frequency: 659.25, offset: 0.13, duration: 0.22 },
  ],
  "update-ready": [
    { frequency: 261.63, offset: 0.00, duration: 0.16 },
    { frequency: 329.63, offset: 0.13, duration: 0.16 },
    { frequency: 392.00, offset: 0.26, duration: 0.28 },
  ],
};

export function cueFrequencies(cue: SoundCue): number[] {
  return CONTOURS[cue].map((tone) => tone.frequency);
}

type AudioContextConstructor = new () => AudioContext;

function audioContextConstructor(): AudioContextConstructor | undefined {
  if (typeof window === "undefined") return undefined;
  const candidate = window as Window & { webkitAudioContext?: AudioContextConstructor };
  return window.AudioContext ?? candidate.webkitAudioContext;
}

/**
 * A restrained Web Audio player shared by every desktop cue. Each note blends a sine and a quiet
 * triangle voice through one low-pass filter and a short attack/release envelope.
 */
export class WebAudioSoundPlayer implements SoundPlayer {
  private context: AudioContext | null = null;

  play(cue: SoundCue, volume: number): boolean {
    try {
      const Context = audioContextConstructor();
      if (!Context) return false;
      const context = this.context ?? new Context();
      this.context = context;
      if (context.state === "closed") return false;
      if (context.state === "suspended") void context.resume().catch(() => undefined);

      const level = Math.max(0, Math.min(1, volume));
      if (level === 0) return true;
      const filter = context.createBiquadFilter();
      filter.type = "lowpass";
      filter.frequency.value = 1800;
      filter.Q.value = 0.45;
      filter.connect(context.destination);

      const base = context.currentTime + 0.015;
      let finalStop = base;
      for (const tone of CONTOURS[cue]) {
        const start = base + tone.offset;
        const stop = start + tone.duration;
        finalStop = Math.max(finalStop, stop);
        const envelope = context.createGain();
        const peak = Math.max(0.0001, level * 0.16);
        envelope.gain.setValueAtTime(0.0001, start);
        envelope.gain.exponentialRampToValueAtTime(peak, start + 0.025);
        envelope.gain.exponentialRampToValueAtTime(0.0001, stop);
        envelope.connect(filter);

        for (const [type, mix] of [["sine", 0.72], ["triangle", 0.28]] as const) {
          const voice = context.createOscillator();
          const voiceGain = context.createGain();
          voice.type = type;
          voice.frequency.setValueAtTime(tone.frequency, start);
          voiceGain.gain.value = mix;
          voice.connect(voiceGain);
          voiceGain.connect(envelope);
          voice.start(start);
          voice.stop(stop + 0.02);
        }
      }
      window.setTimeout(() => filter.disconnect(), Math.max(1, (finalStop - context.currentTime + 0.1) * 1000));
      return true;
    } catch {
      return false;
    }
  }
}

export const desktopSoundPlayer: SoundPlayer = new WebAudioSoundPlayer();
