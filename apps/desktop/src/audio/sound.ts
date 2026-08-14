export const SOUND_CUES = [
  "agent-needs-you",
  "agent-finished",
  "repomind-needs-you",
  "error-or-stall",
  "incoming-message",
  "update-ready",
] as const;

export type SoundCue = typeof SOUND_CUES[number];

export type SoundProfile = "modern" | "bell" | "marimba" | "synth" | "minimal";

export interface SoundProfileMeta {
  id: SoundProfile;
  name: string;
  description: string;
}

export const SOUND_PROFILES: SoundProfileMeta[] = [
  { id: "modern", name: "Modern Chime", description: "Balanced harmonic chime with soft attack" },
  { id: "bell", name: "Warm Bell", description: "Resonant acoustic bell with rich sustained undertones" },
  { id: "marimba", name: "Crisp Marimba", description: "Snappy wooden percussive acoustic strikes" },
  { id: "synth", name: "Tactical Synth", description: "Futuristic crisp electronic telemetry cues" },
  { id: "minimal", name: "Subtle Pop", description: "Discreet and unobtrusive micro-blips" },
];

const soundProfileStorageKey = "repomon-sound-profile";

export function readSoundProfile(): SoundProfile {
  if (typeof window === "undefined") return "modern";
  const saved = window.localStorage.getItem(soundProfileStorageKey);
  return SOUND_PROFILES.some((p) => p.id === saved) ? (saved as SoundProfile) : "modern";
}

export function saveSoundProfile(profile: SoundProfile): void {
  if (typeof window === "undefined") return;
  window.localStorage.setItem(soundProfileStorageKey, profile);
}

export interface SoundPlayer {
  /** Returns true when custom audio was scheduled successfully. */
  play(cue: SoundCue, volume: number, profile?: SoundProfile): boolean;
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
 * A versatile Web Audio player that supports multiple sound design profiles.
 */
export class WebAudioSoundPlayer implements SoundPlayer {
  private context: AudioContext | null = null;

  play(cue: SoundCue, volume: number, profile?: SoundProfile): boolean {
    try {
      const Context = audioContextConstructor();
      if (!Context) return false;
      const context = this.context ?? new Context();
      this.context = context;
      if (context.state === "closed") return false;
      if (context.state === "suspended") void context.resume().catch(() => undefined);

      const level = Math.max(0, Math.min(1, volume));
      if (level === 0) return true;

      const activeProfile = profile ?? readSoundProfile();

      const filter = context.createBiquadFilter();
      if (activeProfile === "synth") {
        filter.type = "bandpass";
        filter.frequency.value = 2400;
        filter.Q.value = 1.2;
      } else if (activeProfile === "minimal") {
        filter.type = "lowpass";
        filter.frequency.value = 3200;
        filter.Q.value = 0.2;
      } else if (activeProfile === "bell") {
        filter.type = "lowpass";
        filter.frequency.value = 2200;
        filter.Q.value = 0.6;
      } else if (activeProfile === "marimba") {
        filter.type = "bandpass";
        filter.frequency.value = 1400;
        filter.Q.value = 0.8;
      } else {
        filter.type = "lowpass";
        filter.frequency.value = 1800;
        filter.Q.value = 0.45;
      }
      filter.connect(context.destination);

      const base = context.currentTime + 0.015;
      let finalStop = base;

      for (const tone of CONTOURS[cue]) {
        const factor = activeProfile === "minimal" ? 0.4 : activeProfile === "bell" ? 1.4 : 1.0;
        const duration = tone.duration * factor;
        const start = base + (tone.offset * (activeProfile === "minimal" ? 0.6 : 1.0));
        const stop = start + duration;
        finalStop = Math.max(finalStop, stop);

        const envelope = context.createGain();
        const peak = Math.max(0.0001, level * (activeProfile === "synth" ? 0.12 : 0.16));

        envelope.gain.setValueAtTime(0.0001, start);
        if (activeProfile === "marimba") {
          envelope.gain.linearRampToValueAtTime(peak, start + 0.005);
          envelope.gain.exponentialRampToValueAtTime(0.0001, stop);
        } else if (activeProfile === "minimal") {
          envelope.gain.linearRampToValueAtTime(peak, start + 0.008);
          envelope.gain.exponentialRampToValueAtTime(0.0001, stop);
        } else {
          envelope.gain.exponentialRampToValueAtTime(peak, start + 0.025);
          envelope.gain.exponentialRampToValueAtTime(0.0001, stop);
        }
        envelope.connect(filter);

        let voices: [OscillatorType, number][] = [["sine", 0.72], ["triangle", 0.28]];
        if (activeProfile === "synth") {
          voices = [["sawtooth", 0.4], ["square", 0.3], ["sine", 0.3]];
        } else if (activeProfile === "bell") {
          voices = [["sine", 0.6], ["triangle", 0.2], ["sine", 0.2]];
        } else if (activeProfile === "minimal") {
          voices = [["sine", 0.9], ["triangle", 0.1]];
        }

        for (const [type, mix] of voices) {
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
