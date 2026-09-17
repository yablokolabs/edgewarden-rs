# Tiny TCP echo upstream for the demo fleet (no external images needed).
FROM python:3.12-slim
RUN useradd -r -u 10001 edge
COPY deploy/docker/echo-upstream.py /srv/echo.py
USER 10001
EXPOSE 18081
CMD ["python3", "/srv/echo.py"]
