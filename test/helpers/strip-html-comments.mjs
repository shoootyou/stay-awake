export const stripHtmlComments = (source) => {
  let stripped = source;
  while (true) {
    const start = stripped.indexOf("<!--");
    if (start === -1) return stripped;
    const end = stripped.indexOf("-->", start + "<!--".length);
    stripped = end === -1
      ? stripped.slice(0, start)
      : stripped.slice(0, start) + stripped.slice(end + "-->".length);
  }
};
