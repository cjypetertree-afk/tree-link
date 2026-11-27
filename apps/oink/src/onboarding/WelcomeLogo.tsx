export function WelcomeLogo({ leaving }: { leaving: boolean }) {
  return (
    <div className="ob-logo" data-leaving={leaving ? '' : undefined} role="img" aria-label="TREE LINK">
      <svg className="ob-logo__mark" viewBox="0 0 100 100" aria-hidden>
        <ellipse cx="50" cy="86" rx="22" ry="4.5" fill="#07110D" opacity="0.28" />
        <path d="M42.5 59 C42.5 70 39.5 77.5 36.5 82.5 C35.5 84.5 37.5 86 40.5 86 H59.5 C62.5 86 64.5 84.5 63.5 82.5 C60.5 77.5 57.5 70 57.5 59 Z" fill="#C47E4A" />
        <path d="M45.5 61.5 C45.5 70 43.2 76.8 40.8 82" fill="none" stroke="#E2A474" strokeWidth="2.8" strokeLinecap="round" />
        <circle cx="31" cy="52" r="17.5" fill="#15803D" />
        <circle cx="69" cy="52" r="17.5" fill="#15803D" />
        <circle cx="50" cy="54" r="21.5" fill="#15803D" />
        <circle cx="31.5" cy="46.5" r="17" fill="#22C55E" />
        <circle cx="68.5" cy="46.5" r="17" fill="#22C55E" />
        <circle cx="50" cy="34.5" r="20.5" fill="#22C55E" />
        <circle cx="50" cy="48.5" r="20.5" fill="#22C55E" />
        <circle cx="43.5" cy="27" r="11" fill="#86EFAC" opacity="0.55" />
        <path d="M50 15.5 Q51.5 7.5 59.5 5.5 Q58 13.5 50 15.5 Z" fill="#86EFAC" />
        <path d="M50 15.5 Q47.5 8.5 40.5 7.5 Q42.5 14 50 15.5 Z" fill="#4ADE80" />
        <circle cx="40.5" cy="48.5" r="3.6" fill="#0F241B" />
        <circle cx="39.2" cy="47.2" r="1.25" fill="#FFFFFF" />
        <circle cx="59.5" cy="48.5" r="3.6" fill="#0F241B" />
        <circle cx="58.2" cy="47.2" r="1.25" fill="#FFFFFF" />
        <ellipse cx="33" cy="53.5" rx="3.7" ry="2.1" fill="#FDA4AF" opacity="0.92" />
        <ellipse cx="67" cy="53.5" rx="3.7" ry="2.1" fill="#FDA4AF" opacity="0.92" />
        <path d="M46.2 52.8 Q50 56.8 53.8 52.8" fill="none" stroke="#0F241B" strokeWidth="2.5" strokeLinecap="round" />
        <circle cx="23.5" cy="26.5" r="2.8" fill="#38BDF8" />
        <circle cx="77" cy="29.5" r="2.4" fill="#FDE047" />
      </svg>
      <svg className="ob-logo__word" viewBox="0 0 260 64" aria-hidden>
        <text x="6" y="46" fill="currentColor" fontSize="38" fontWeight="800" letterSpacing="3" fontFamily="-apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif">TREE LINK</text>
      </svg>
    </div>
  );
}
