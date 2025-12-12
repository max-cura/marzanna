import torch
import matplotlib.pyplot as plt
import polars as pl
import torch.nn as nn
import torch.nn.functional as F
import torch.optim as optim
import numpy as np

K = 25


def load_jsonl(file: str, label) -> [torch.Tensor, torch.Tensor]:
    """
    returns ([N, 25, 2], [N])
    """
    df = pl.read_ndjson(source=file)
    print(f"Loaded {file}")
    df = df.group_by('src').agg(
            pl.col('domain').count().alias('reqs'),
            pl.col('domain')
              .str.slice(0, 16)
              .str.to_integer(base=16, dtype=pl.UInt64)
              .reinterpret(signed=True),
            pl.col('time').cast(pl.Int64)
        ) \
        .filter(
            pl.col('reqs') >= K
        ) \
        .with_columns(
            pl.col('domain').list.slice(0, K).list.to_array(width=K),
            pl.col('time').list.slice(0, K).list.to_array(width=K),
            pl.lit(label).alias('label').cast(pl.Int64)
        )
    domains = df['domain'].to_numpy()
    time = df['time'].to_numpy()
    torch_dt = torch.tensor(np.stack((domains, time), -1))
    torch_labels = df['label'].to_torch()
    print(f'values: {torch_dt.shape}')
    print(f'labels: {torch_labels.shape}')
    return (torch_dt, torch_labels)


ben_x, ben_y = load_jsonl("../run-dnstrace-3600.1.jsonl", 0)  # 66488
mz11_x, mz11_y = load_jsonl("../sst2_d10_rate_1_chunk25.jsonl", 1)  # 110
mz5a_x, mz5a_y = load_jsonl("../sst1_d10_rate_5_10_chunk25.jsonl", 1)  # 57
mz36_x, mz36_y = load_jsonl("../sst1_d10_rate_3_6_chunk25.jsonl", 1)  # 60


def splits(s, t, a, b, c):
    train = s[0:a], t[0:a]
    valid = s[a:b], t[a:b]
    test = s[b:c], t[b:c]
    return train, valid, test


class LSTMClassifier(nn.Module):
    def __init__(self, vocab_size, embed_dim, hidden_dim, num_layers, output_size, dropout=0.0):
        super(LSTMClassifier, self).__init__()
        self.embed = nn.Embedding(vocab_size, embed_dim)
        self.lstm = nn.LSTM(embed_dim + 1,
                            hidden_dim,
                            num_layers,
                            batch_first=True,
                            dropout=dropout)
        self.fc0 = nn.Linear(hidden_dim, 30)
        self.fc1 = nn.Linear(30, 10)
        self.fc2 = nn.Linear(10, output_size)

    def forward(self, x):
        embed_d = self.embed(x[:, :, 0])
        embed_t = torch.unsqueeze(x[:, :, 1].float(), 2)
        embedded = torch.cat((embed_d, embed_t), 2)
        out, (h, c) = self.lstm(embedded)
        # res = self.fc(h[-1])
        f0 = self.fc0(out[:, -1, :])
        f1 = self.fc1(F.relu(f0))
        f2 = self.fc2(F.relu(f1))
        # res = F.sigmoid(out2)
        # print("FC:", res.shape)
        return f2


def train(mz_x, mz_y, sa, sb):
    ben_train, ben_valid, ben_test = splits(ben_x, ben_y, 500, 570, 640)
    mz_train, mz_valid, mz_test = splits(mz_x, mz_y, sa, sb, len(mz_x))
    full_x = torch.cat((ben_x, mz_x))
    unique_domains = torch.unique(full_x[:, :, 0])
    d = {k: i for i, k in enumerate(unique_domains.tolist())}

    def recat(v):
        # [N, 25, 2] -> [N, 25, 2]
        # print('uniques: ', unique_domains)
        for i in range(0, v.shape[0]):
            for j in range(0, v.shape[1]):
                v[i, j, 0] = d[v[i, j, 0].item()]
        return v

    def xy(s, t):
        x = torch.cat((s[0], t[0]))
        y = torch.cat((s[1], t[1]))
        return recat(x), y

    train_x, train_y = xy(ben_train, mz_train)
    valid_x, valid_y = xy(ben_valid, mz_valid)
    test_x, test_y = xy(ben_test, mz_test)

    print("Building classifier")

    model = LSTMClassifier(
        vocab_size=len(d),
        embed_dim=31,
        hidden_dim=64,
        num_layers=2,
        output_size=2,
        dropout=0.0
    )
    crit = nn.CrossEntropyLoss()
    opt = optim.Adam(model.parameters(), lr=0.001)

    print("Running")

    num_epochs = 100
    for epoch in range(num_epochs):
        opt.zero_grad()
        outputs = model(train_x)
        loss = crit(outputs, train_y)
        loss.backward()
        opt.step()
        print(f'epoch {epoch+1}/{num_epochs}: loss is {loss.item()}')

    with torch.no_grad():
        print("test_x: ", test_x.shape)
        pred = model(test_x)
        print("pred: ", pred)
        pred_lbl = torch.argmax(pred, dim=1)
        TP = 0
        FP = 0
        TN = 0
        FN = 0
        NA = 0
        print("pred_lbl: ", pred_lbl)
        print("real_lbl: ", test_y)
        for lbl, real in zip(pred_lbl.tolist(), test_y.tolist()):
            # lbl = lbl.item()
            # real = real.item()
            if lbl == 1 and real == 1:
                TP += 1
            elif lbl == 1 and real == 0:
                FP += 1
            elif lbl == 0 and real == 0:
                TN += 1
            elif lbl == 0 and real == 1:
                FN += 1
            else:
                NA += 1
        print(f"TP={TP}, FP={FP}, TN={TN}, FN={FN}, NA={NA}")


train(mz11_x, mz11_y, 77, 93)

# def main():
#     benign_df = pl.read_ndjson(source='../run-dnstrace-3600.1.jsonl')
#     flag_df = pl.read_ndjson(source='../sst2_d10_rate_1.jsonl')
#     print(benign_df)
#     print(flag_df)

# if __name__ == "__main__":
#     main()
