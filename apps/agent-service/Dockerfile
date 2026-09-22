FROM python:3.12-slim
WORKDIR /app
COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt
COPY agent.py workflows.py ./
RUN useradd -r -u 10001 aether
USER 10001
CMD ["python", "-c", "import agent; print('agent graph ready')"]
