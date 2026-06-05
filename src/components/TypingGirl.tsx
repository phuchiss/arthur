import { useEffect, useRef } from "react";
import lottie, { type AnimationItem } from "lottie-web/build/player/lottie_light";
import animationData from "../assets/typing-animation.json";

// Animation: "Sleeping" by DU AMV — https://lottiefiles.com/free-animation/sleeping-duJHnWdTuw
// License: Lottie Simple License (free for personal and commercial use).

type Props = { busy: boolean };

const REDUCED_MOTION_QUERY = "(prefers-reduced-motion: reduce)";

export function TypingGirl({ busy }: Props) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const animRef = useRef<AnimationItem | null>(null);

  useEffect(() => {
    if (!containerRef.current) return;
    const anim = lottie.loadAnimation({
      container: containerRef.current,
      renderer: "svg",
      loop: true,
      autoplay: false,
      animationData,
    });
    animRef.current = anim;
    return () => {
      anim.destroy();
      animRef.current = null;
    };
  }, []);

  useEffect(() => {
    const anim = animRef.current;
    if (!anim) return;
    if (busy) {
      const reduced =
        typeof window !== "undefined" &&
        typeof window.matchMedia === "function" &&
        window.matchMedia(REDUCED_MOTION_QUERY).matches;
      if (reduced) {
        anim.goToAndStop(0, true);
      } else {
        anim.play();
      }
    } else {
      anim.pause();
    }
  }, [busy]);

  return (
    <>
      <div
        className={`typing-girl${busy ? " is-visible" : ""}`}
        aria-hidden="true"
      >
        <div ref={containerRef} className="typing-girl__anim" />
      </div>
      <span className="sr-only" role="status">
        {busy ? "Assistant is working" : ""}
      </span>
    </>
  );
}
