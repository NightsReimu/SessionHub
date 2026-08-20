import { useEffect, useRef } from "react";

interface Particle {
  x: number;
  y: number;
  vx: number;
  vy: number;
  life: number;
  decay: number;
  size: number;
  hue: number;
}

/** 彩色鼠标轨迹：全屏 canvas，粒子随移动生成、色相循环、发光淡出 */
export default function MouseTrail() {
  const ref = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const canvas = ref.current!;
    const ctx = canvas.getContext("2d")!;
    let raf = 0;
    let hue = Math.random() * 360;
    const particles: Particle[] = [];
    let last = { x: -100, y: -100 };

    const resize = () => {
      const dpr = window.devicePixelRatio || 1;
      canvas.width = window.innerWidth * dpr;
      canvas.height = window.innerHeight * dpr;
      canvas.style.width = window.innerWidth + "px";
      canvas.style.height = window.innerHeight + "px";
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    };
    resize();
    window.addEventListener("resize", resize);

    const onMove = (e: MouseEvent) => {
      const dx = e.clientX - last.x;
      const dy = e.clientY - last.y;
      const dist = Math.hypot(dx, dy);
      if (dist < 3) return;
      hue = (hue + dist * 0.5) % 360;
      const steps = Math.min(4, Math.floor(dist / 6) + 1);
      for (let i = 0; i < steps; i++) {
        const t = i / steps;
        particles.push({
          x: last.x + dx * t,
          y: last.y + dy * t,
          vx: (Math.random() - 0.5) * 0.6,
          vy: (Math.random() - 0.5) * 0.6 - 0.15,
          life: 1,
          decay: 0.012 + Math.random() * 0.016,
          size: 3 + Math.random() * 5,
          hue: (hue + Math.random() * 24 - 12 + 360) % 360,
        });
      }
      // 粒子总量封顶，保证长时间使用不拖性能
      if (particles.length > 350) particles.splice(0, particles.length - 350);
      last = { x: e.clientX, y: e.clientY };
    };
    window.addEventListener("mousemove", onMove);

    const tick = () => {
      ctx.clearRect(0, 0, window.innerWidth, window.innerHeight);
      ctx.globalCompositeOperation = "lighter";
      for (let i = particles.length - 1; i >= 0; i--) {
        const p = particles[i];
        p.x += p.vx;
        p.y += p.vy;
        p.life -= p.decay;
        if (p.life <= 0) {
          particles.splice(i, 1);
          continue;
        }
        const alpha = p.life * 0.7;
        const radius = p.size * p.life * 2.2;
        const g = ctx.createRadialGradient(p.x, p.y, 0, p.x, p.y, radius);
        g.addColorStop(0, `hsla(${p.hue}, 95%, 66%, ${alpha})`);
        g.addColorStop(1, `hsla(${p.hue}, 95%, 55%, 0)`);
        ctx.fillStyle = g;
        ctx.beginPath();
        ctx.arc(p.x, p.y, radius, 0, Math.PI * 2);
        ctx.fill();
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);

    return () => {
      cancelAnimationFrame(raf);
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("resize", resize);
    };
  }, []);

  return <canvas ref={ref} className="pointer-events-none fixed inset-0 z-40" />;
}
